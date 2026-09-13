# Development rules

Use strict TDD: behavioral and accessible UI contracts first, unit tests second,
integration tests third, implementation last. Record the failing check before fixing it.
Keep changes small and reversible. Run `make check` after every change (Windows:
`python scripts/check.py`). Never report a failing check or an untested platform as done.
Never commit credentials, device identities, or environment-specific infrastructure.
Release packages must identify the exact tested source commit and version.
Use supported platform APIs and configuration. Do not add workaround paths that
bypass authentication, VM isolation, owner controls, or health qualification.
Report unsupported host configurations clearly; preserve the host's settings.
Do not change, disconnect, reconnect, or reconfigure the host's VPN, or add routing
exceptions to make a worker function. Compatibility with the existing VPN policy
is a product requirement.
