use clap::{Parser, ValueEnum};
use log::{debug, LevelFilter};
use regex::Regex;
use simple_logger::SimpleLogger;
use std::collections::HashMap;
use std::io::{self, BufRead, Read, Write};
use std::process::{Command, Stdio};

/// Separates the columns of one `git format-rev` record.
const COLUMN_SEPARATOR: char = '\u{01}';
/// Separates one decoration from the next, e.g. the `, ` in `(main, origin/main)`.
const DECORATION_SEPARATOR: char = '\u{1f}';
/// Separates `HEAD` from the branch it points at, e.g. the ` -> ` in `(HEAD -> main)`.
const DECORATION_POINTER: char = '\u{1e}';
/// Marks a decoration as a tag.
const DECORATION_TAG: char = '\u{1d}';

/// 🪄 One `git format-rev` invocation yields every column for every row.
const FORMAT: &str = concat!(
    "%H%x01",
    "%(decorate:prefix=,suffix=,separator=%x1f,pointer=%x1e,tag=%x1d)",
    "%x01%s"
);

const MESSAGE_UNAVAILABLE: &str = "Commit message not available";

#[derive(Parser, Debug)]
#[command(name = "test-results")]
struct Opt
{
    #[arg(long)]
    debug: bool,

    #[arg(long, value_enum, default_value_t = Color::Always)]
    color: Color,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Color
{
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefKind
{
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
    Other,
}

impl RefKind
{
    /// Exactly the escapes `git log --color=always` emits for each ref kind.
    fn escape(self) -> &'static str
    {
        match self
        {
            RefKind::Head => "\u{1b}[1;36m",
            RefKind::LocalBranch => "\u{1b}[1;32m",
            RefKind::RemoteBranch => "\u{1b}[1;31m",
            RefKind::Tag => "\u{1b}[1;33m",
            RefKind::Other => "\u{1b}[1;34m",
        }
    }
}

/// Maps a shortened ref name back to its kind, since `%(decorate)` drops the `refs/` prefix
/// and a local branch is free to be named `origin/main`.
struct RefClassifier
{
    kinds: HashMap<String, RefKind>,
}

impl RefClassifier
{
    fn from_for_each_ref(output: &str) -> Self
    {
        let mut kinds = HashMap::new();
        for full_name in output.lines().filter(|line| !line.is_empty())
        {
            let classified = ["refs/heads/", "refs/remotes/", "refs/tags/"]
                .iter()
                .zip([RefKind::LocalBranch, RefKind::RemoteBranch, RefKind::Tag])
                .find_map(|(prefix, kind)| {
                    full_name.strip_prefix(prefix).map(|short| (short, kind))
                });
            if let Some((short_name, kind)) = classified
            {
                kinds.insert(short_name.to_string(), kind);
            }
        }
        RefClassifier { kinds }
    }

    fn load() -> Self
    {
        let output = Command::new("git")
            .args(["for-each-ref", "--format=%(refname)"])
            .output()
            .expect("failed to run git for-each-ref");
        Self::from_for_each_ref(&String::from_utf8_lossy(&output.stdout))
    }

    fn classify(&self, name: &str) -> RefKind
    {
        if name == "HEAD"
        {
            return RefKind::Head;
        }
        *self.kinds.get(name).unwrap_or(&RefKind::Other)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct DecorationRef
{
    name: String,
    is_tag: bool,
}

/// The refs pointing at one commit. Outer groups render separated by `, `,
/// the refs inside a group by ` -> `.
#[derive(Debug, PartialEq, Eq)]
struct Decoration
{
    groups: Vec<Vec<DecorationRef>>,
}

impl Decoration
{
    fn parse(raw: &str) -> Self
    {
        if raw.is_empty()
        {
            return Decoration { groups: Vec::new() };
        }
        let groups = raw
            .split(DECORATION_SEPARATOR)
            .map(|group| {
                group
                    .split(DECORATION_POINTER)
                    .map(|name| match name.strip_prefix(DECORATION_TAG)
                    {
                        Some(tag_name) => DecorationRef {
                            name: tag_name.to_string(),
                            is_tag: true,
                        },
                        None => DecorationRef {
                            name: name.to_string(),
                            is_tag: false,
                        },
                    })
                    .collect()
            })
            .collect();
        Decoration { groups }
    }

    fn render(&self, classifier: &RefClassifier, color: Color) -> String
    {
        if self.groups.is_empty()
        {
            return String::new();
        }

        // 🎨 git paints the punctuation yellow and each ref by its kind.
        let punctuation = |text: &str| match color
        {
            Color::Always => format!("\u{1b}[33m{}\u{1b}[m", text),
            Color::Never => text.to_string(),
        };
        let paint = |text: &str, kind: RefKind| match color
        {
            Color::Always => format!("{}{}\u{1b}[m", kind.escape(), text),
            Color::Never => text.to_string(),
        };

        let mut rendered = punctuation(" (");
        for (group_index, group) in self.groups.iter().enumerate()
        {
            if group_index > 0
            {
                rendered.push_str(&punctuation(", "));
            }
            for (ref_index, reference) in group.iter().enumerate()
            {
                if ref_index > 0
                {
                    rendered.push_str(&punctuation(" -> "));
                }
                if reference.is_tag
                {
                    rendered.push_str(&paint("tag: ", RefKind::Tag));
                    rendered.push_str(&paint(&reference.name, RefKind::Tag));
                }
                else
                {
                    rendered.push_str(&paint(
                        &reference.name,
                        classifier.classify(&reference.name),
                    ));
                }
            }
        }
        rendered.push_str(&punctuation(")"));
        rendered
    }
}

#[derive(Debug)]
struct CommitColumns
{
    decoration: Decoration,
    subject: String,
}

/// A row of input, already matched up with its commit.
#[derive(Debug)]
enum Row
{
    /// A line that carried no commit object name; echoed unchanged.
    Passthrough(String),
    Commit
    {
        line: String, message: String
    },
}

/// 🧭 `git format-rev` silently drops revisions it cannot resolve, so the returned records
/// are matched back to the requested revisions by their `%H` rather than by position.
fn align_columns(requested: &[String], returned: &str) -> Vec<Option<CommitColumns>>
{
    let mut columns: Vec<Option<CommitColumns>> = Vec::with_capacity(requested.len());
    let mut records = returned.lines().peekable();

    for revision in requested
    {
        let matched = records
            .peek()
            .is_some_and(|record| record.split(COLUMN_SEPARATOR).next() == Some(revision.as_str()));

        if !matched
        {
            columns.push(None);
            continue;
        }

        let record = records.next().unwrap();
        let mut fields = record.split(COLUMN_SEPARATOR);
        fields.next();
        let decoration = Decoration::parse(fields.next().unwrap_or(""));
        let subject = fields.next().unwrap_or("").to_string();
        columns.push(Some(CommitColumns {
            decoration,
            subject,
        }));
    }

    columns
}

fn fetch_columns(revisions: &[String]) -> Vec<Option<CommitColumns>>
{
    if revisions.is_empty()
    {
        return Vec::new();
    }

    let mut child = Command::new("git")
        .args(["format-rev", "--stdin-mode=revs"])
        .arg(format!("--format={}", FORMAT))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to run git format-rev");

    // Feed stdin from another thread so a full pipe buffer cannot deadlock against stdout.
    let mut stdin = child.stdin.take().unwrap();
    let payload = revisions.join("\n") + "\n";
    let writer = std::thread::spawn(move || stdin.write_all(payload.as_bytes()));

    let mut returned = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut returned)
        .expect("failed to read git format-rev output");
    writer.join().unwrap().expect("failed to write revisions");
    child.wait().expect("git format-rev failed");

    align_columns(revisions, &returned)
}

/// Width as the terminal sees it: `git test results --color` feeds us lines
/// carrying ANSI escapes, which occupy no columns.
fn display_width(line: &str) -> usize
{
    let mut width = 0;
    let mut characters = line.chars();
    while let Some(character) = characters.next()
    {
        if character == '\u{1b}'
        {
            for escape_character in characters.by_ref()
            {
                if escape_character.is_ascii_alphabetic()
                {
                    break;
                }
            }
        }
        else
        {
            width += 1;
        }
    }
    width
}

/// 📏 Every row is padded to the widest line, which is only knowable once all input is read.
fn render_rows(rows: &[Row]) -> String
{
    let width = rows
        .iter()
        .filter_map(|row| match row
        {
            Row::Commit { line, .. } => Some(display_width(line)),
            Row::Passthrough(_) => None,
        })
        .max()
        .unwrap_or(0);

    let mut rendered = String::new();
    for row in rows
    {
        match row
        {
            Row::Passthrough(line) =>
            {
                rendered.push_str(line);
            }
            Row::Commit { line, message } =>
            {
                rendered.push_str(line);
                let padding = width.saturating_sub(display_width(line));
                rendered.extend(std::iter::repeat_n(' ', padding + 1));
                rendered.push_str(message);
            }
        }
        rendered.push('\n');
    }
    rendered
}

fn process_git_log<R: BufRead, W: Write>(reader: R, writer: &mut W, color: Color)
    -> io::Result<()>
{
    let sha_regex: Regex = Regex::new(r"^[a-f0-9]{40}").unwrap();

    let mut lines: Vec<String> = Vec::new();
    let mut revisions: Vec<String> = Vec::new();

    for line in reader.lines()
    {
        let line = line?;
        if let Some(matched) = sha_regex.find(&line)
        {
            revisions.push(matched.as_str().to_string());
        }
        lines.push(line);
    }

    debug!("Formatting {} revisions in one batch", revisions.len());
    let columns = fetch_columns(&revisions);
    let classifier = RefClassifier::load();

    let mut columns = columns.into_iter();
    let rows: Vec<Row> = lines
        .into_iter()
        .map(|line| {
            if !sha_regex.is_match(&line)
            {
                return Row::Passthrough(line);
            }
            let message = match columns.next().unwrap()
            {
                Some(commit) => format!(
                    "{} {}",
                    commit.decoration.render(&classifier, color),
                    commit.subject
                ),
                None => MESSAGE_UNAVAILABLE.to_string(),
            };
            Row::Commit { line, message }
        })
        .collect();

    write!(writer, "{}", render_rows(&rows))?;
    writer.flush()
}

fn setup_logging(debug: bool)
{
    let level = if debug
    {
        LevelFilter::Debug
    }
    else
    {
        LevelFilter::Info
    };
    SimpleLogger::new().with_level(level).init().unwrap();
}

fn main() -> io::Result<()>
{
    let opt = Opt::parse_from(std::env::args());
    setup_logging(opt.debug);

    debug!("Starting git log processing");
    let stdin = io::stdin();
    let stdout = io::stdout();
    process_git_log(stdin.lock(), &mut stdout.lock(), opt.color)?;
    debug!("Finished processing git log");
    Ok(())
}

#[cfg(test)]
mod tests
{
    use super::*;

    fn classifier() -> RefClassifier
    {
        RefClassifier::from_for_each_ref(
            "refs/heads/zizmor-fixes\n\
             refs/heads/sync-template\n\
             refs/remotes/origin/main\n\
             refs/remotes/origin/HEAD\n\
             refs/tags/v1.0\n",
        )
    }

    // 🎨 Ground truth captured from `git log --color=always`
    #[test]
    fn renders_head_pointing_at_local_branch()
    {
        let decoration = Decoration::parse("HEAD\u{1e}zizmor-fixes");
        assert_eq!(
            decoration.render(&classifier(), Color::Always),
            "\u{1b}[33m (\u{1b}[m\u{1b}[1;36mHEAD\u{1b}[m\u{1b}[33m -> \u{1b}[m\
             \u{1b}[1;32mzizmor-fixes\u{1b}[m\u{1b}[33m)\u{1b}[m"
        );
    }

    #[test]
    fn renders_remote_branches_in_bold_red()
    {
        let decoration = Decoration::parse("origin/main\u{1f}origin/HEAD\u{1f}sync-template");
        assert_eq!(
            decoration.render(&classifier(), Color::Always),
            "\u{1b}[33m (\u{1b}[m\u{1b}[1;31morigin/main\u{1b}[m\u{1b}[33m, \u{1b}[m\
             \u{1b}[1;31morigin/HEAD\u{1b}[m\u{1b}[33m, \u{1b}[m\
             \u{1b}[1;32msync-template\u{1b}[m\u{1b}[33m)\u{1b}[m"
        );
    }

    #[test]
    fn renders_tag_with_its_prefix_separately_wrapped()
    {
        let decoration = Decoration::parse("HEAD\u{1e}zizmor-fixes\u{1f}\u{1d}v1.0");
        assert_eq!(
            decoration.render(&classifier(), Color::Always),
            "\u{1b}[33m (\u{1b}[m\u{1b}[1;36mHEAD\u{1b}[m\u{1b}[33m -> \u{1b}[m\
             \u{1b}[1;32mzizmor-fixes\u{1b}[m\u{1b}[33m, \u{1b}[m\
             \u{1b}[1;33mtag: \u{1b}[m\u{1b}[1;33mv1.0\u{1b}[m\u{1b}[33m)\u{1b}[m"
        );
    }

    #[test]
    fn renders_empty_decoration_as_empty_string()
    {
        assert_eq!(
            Decoration::parse("").render(&classifier(), Color::Always),
            ""
        );
        assert_eq!(
            Decoration::parse("").render(&classifier(), Color::Never),
            ""
        );
    }

    #[test]
    fn renders_without_escapes_when_color_is_never()
    {
        let decoration = Decoration::parse("HEAD\u{1e}zizmor-fixes\u{1f}\u{1d}v1.0");
        assert_eq!(
            decoration.render(&classifier(), Color::Never),
            " (HEAD -> zizmor-fixes, tag: v1.0)"
        );
    }

    // 🧭 A rev that git skips must not shift every later row onto the wrong commit
    #[test]
    fn realigns_columns_when_git_skips_an_unknown_rev()
    {
        let requested = vec![
            "aaaaaaa".to_string(),
            "bbbbbbb".to_string(),
            "ccccccc".to_string(),
        ];
        let returned = "aaaaaaa\u{01}\u{01}first\nccccccc\u{01}\u{01}third\n";

        let columns = align_columns(&requested, returned);

        assert_eq!(columns.len(), 3);
        assert_eq!(columns[0].as_ref().unwrap().subject, "first");
        assert!(columns[1].is_none());
        assert_eq!(columns[2].as_ref().unwrap().subject, "third");
    }

    #[test]
    fn realigns_when_the_very_first_rev_is_skipped()
    {
        let requested = vec!["aaaaaaa".to_string(), "bbbbbbb".to_string()];
        let returned = "bbbbbbb\u{01}\u{01}second\n";

        let columns = align_columns(&requested, returned);

        assert!(columns[0].is_none());
        assert_eq!(columns[1].as_ref().unwrap().subject, "second");
    }

    #[test]
    fn keeps_duplicate_revs_distinct_and_in_order()
    {
        let requested = vec![
            "aaaaaaa".to_string(),
            "bbbbbbb".to_string(),
            "aaaaaaa".to_string(),
        ];
        let returned =
            "aaaaaaa\u{01}\u{01}first\nbbbbbbb\u{01}\u{01}second\naaaaaaa\u{01}\u{01}first\n";

        let columns = align_columns(&requested, returned);

        assert_eq!(columns[0].as_ref().unwrap().subject, "first");
        assert_eq!(columns[1].as_ref().unwrap().subject, "second");
        assert_eq!(columns[2].as_ref().unwrap().subject, "first");
    }

    // 📏 Padding uses every line, not just the ones seen so far
    #[test]
    fn pads_all_rows_to_the_widest_line()
    {
        let rows = vec![
            Row::Commit {
                line: "short^{tree} ok".to_string(),
                message: "one".to_string(),
            },
            Row::Commit {
                line: "a_much_longer_line^{tree} ok".to_string(),
                message: "two".to_string(),
            },
        ];

        assert_eq!(
            render_rows(&rows),
            "short^{tree} ok              one\na_much_longer_line^{tree} ok two\n"
        );
    }

    #[test]
    fn echoes_lines_that_carry_no_commit_unchanged()
    {
        let rows = vec![
            Row::Passthrough("some header".to_string()),
            Row::Commit {
                line: "short^{tree} ok".to_string(),
                message: "one".to_string(),
            },
        ];

        assert_eq!(render_rows(&rows), "some header\nshort^{tree} ok one\n");
    }

    // 🎨 Coloured input from `git test results --color` must still line up
    #[test]
    fn ignores_ansi_escapes_when_measuring_width()
    {
        assert_eq!(display_width("\u{1b}[32mshort\u{1b}[m^{tree} ok"), 15);

        let rows = vec![
            Row::Commit {
                line: "\u{1b}[32mshort\u{1b}[m^{tree} ok".to_string(),
                message: "one".to_string(),
            },
            Row::Commit {
                line: "a_much_longer_line^{tree} ok".to_string(),
                message: "two".to_string(),
            },
        ];

        assert_eq!(
            render_rows(&rows),
            "\u{1b}[32mshort\u{1b}[m^{tree} ok              one\na_much_longer_line^{tree} ok two\n"
        );
    }
}
