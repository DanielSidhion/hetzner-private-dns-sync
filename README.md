Updates entries on a DNS server based on IP assignments given by Hetzner on a private network.
This makes it possible to query hosts on a private network by their internal FQDN (as long as their DNS setup is correct).

Comes with a NixOS module in the flake output to make it easy to use.

# TODO

- [ ] Add timeouts on network calls.
- [ ] Add nix usage examples (+ a DNS server setup).
