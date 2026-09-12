# Development rules

Use strict TDD: behavioral and accessible UI contracts first, unit tests second,
integration tests third, implementation last. Record the failing check before fixing it.
Keep changes small and reversible. Run `make check` after every change (Windows:
`python scripts/check.py`). Never report a failing check or an untested platform as done.
Never commit credentials, device identities, or environment-specific infrastructure.
Release packages must identify the exact tested source commit and version.
