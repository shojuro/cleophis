# Secret scanning

Security review 2026-10-04 (L27). CI runs gitleaks with `.gitleaks.toml`; the same scan is available locally as a pre-commit hook that checks only the staged changes.

## Enable the hook (once per clone)

```
git config core.hooksPath tools/git-hooks
```

`core.hooksPath` is repo-wide, so it applies to every worktree of the clone. It needs `gitleaks` on PATH; without it the hook prints a warning and lets the commit through.
