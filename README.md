This program takes the output of git-test and adds the commit message to each line of output.

Before:

```console
❯ git test results --color upstream/master..HEAD
4e715891d543d883e8518b1f1c3b3c424bbea7b3^{tree} known-good
cc4c667e4b84e67b8e3bd8cabdaad3eaedd651cd^{tree} known-bad
b0f8808cb5b15d20981f7eed0f0ac424d0880238^{tree} unknown
```

After:

```console
❯ git test results --color upstream/master..HEAD | test-results
4e715891d543d883e8518b1f1c3b3c424bbea7b3^{tree} known-good | Add a working feature
cc4c667e4b84e67b8e3bd8cabdaad3eaedd651cd^{tree} known-bad  | Add a bug
b0f8808cb5b15d20981f7eed0f0ac424d0880238^{tree} unknown    | fixup! Add a bug
```
