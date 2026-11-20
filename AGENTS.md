# AGENTS.md

## GitHub Actions: Pages deploys only

- The only workflows allowed in `.github/workflows/` are GitHub Pages deploys
  (`actions/upload-pages-artifact` + `actions/deploy-pages`). Don't add CI, tests, builds,
  release or packaging jobs, scheduled checks, or deploys to anything other than GitHub Pages:
  run those locally.
- Never upload build artifacts (`actions/upload-artifact`). In November 2026 build artifacts
  filled the account's Actions storage and blocked every Pages deploy.
