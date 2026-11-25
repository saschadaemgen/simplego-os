# Server tooling

Tooling for the onion-only SimpleX relays of SimpleGo (SMP for messages, XFTP for files), run from the SimpleGo OS laptop over Tor with post-quantum SSH key exchange.

Status: the scripts in use are being cleaned for publication. They will arrive here parameterised: every server address, onion address, fingerprint and key is supplied on the laptop at run time and never stored in this repository.

What the tooling covers:

| Step | Content |
|:-----|:--------|
| Setup | Debian server reached only through its SSH onion service with client authorization |
| Hardening | nftables with output only through Tor, SSH with post-quantum key exchange, onion service proof-of-work and intro DoS defences |
| Relays | `smp-server` and `xftp-server` from simplexmq, keys and certificates created offline on the laptop (the CA key never reaches the server), automatic rollback unless confirmed |
| Checks | Reachability through Tor like an app, identity compared with the offline record |
