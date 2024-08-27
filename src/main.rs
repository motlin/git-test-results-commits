use log::{debug, error, LevelFilter};
use regex::Regex;
use simple_logger::SimpleLogger;
use std::io::{self, BufRead, Write};
use std::process::Command;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
#[structopt(name = "git-log-formatter")]
struct Opt
{
    #[structopt(long)]
    debug: bool,
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

fn get_commit_message(treeish: &str) -> String
{
    let commit_sha: String = treeish.replace("^{tree}", "");
    debug!("Fetching commit message for SHA: {}", commit_sha);

    let command = format!("git log --format=%B -n 1 {}", commit_sha);
    debug!("Running command: {}", command);

    match Command::new("sh").arg("-c").arg(&command).output()
    {
        Ok(output) =>
        {
            if output.status.success()
            {
                let message = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .chars()
                    .take(120)
                    .collect::<String>();
                debug!(
                    "Commit message retrieved: {}...",
                    &message.chars().take(120).collect::<String>()
                );
                message
            }
            else
            {
                error!(
                    "Error fetching commit message for {}: {}",
                    treeish,
                    String::from_utf8_lossy(&output.stderr)
                );
                "Commit message not available".to_string()
            }
        }
        Err(e) =>
        {
            error!("Failed to execute git command for {}: {}", treeish, e);
            "Commit message not available".to_string()
        }
    }
}

fn process_git_log<R: BufRead, W: Write>(reader: R, writer: &mut W) -> io::Result<()>
{
    let sha_regex: Regex = Regex::new(r"^[a-f0-9]{40}").unwrap();

    let mut max_line_length = 0;

    for line in reader.lines()
    {
        let line: String = line?;
        max_line_length = max_line_length.max(line.len());

        if sha_regex.is_match(&line)
        {
            let treeish: &str = line.split_whitespace().next().unwrap();
            let commit_message: String = get_commit_message(treeish);
            writeln!(
                writer,
                "{:<width$} | {}",
                line,
                commit_message,
                width = max_line_length
            )?;
        }
        else
        {
            writeln!(writer, "{}", line)?;
        }
        writer.flush()?;
    }

    Ok(())
}

fn main() -> io::Result<()>
{
    let opt = Opt::from_args();
    setup_logging(opt.debug);

    debug!("Starting git log processing");
    let stdin = io::stdin();
    let stdout = io::stdout();
    process_git_log(stdin.lock(), &mut stdout.lock())?;
    debug!("Finished processing git log");
    Ok(())
}
