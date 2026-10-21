# Security

Use GitHub private vulnerability reporting from this repository's Security tab.
If private reporting is unavailable, open an issue requesting a private contact
without including vulnerability details. Never put credentials, confidential
specifications or exploit payloads in a public issue.

The converter treats specifications as data. It never executes collection scripts,
shell commands, resolvers or API operations. The Rust core has no ambient file or
network access. Hosts must supply resources explicitly, bound input/output and
run untrusted workloads in disposable processes or browser workers.

Review reports and exported artifacts before sharing: original source bytes may
contain credentials. See [hosting](README.md) for service isolation and
[conversion support](README.md) for semantics and limitations.
