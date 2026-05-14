# Security Policy

## Supported Versions

The following versions are currently supported with security updates:

| Version | Supported |
| ------- | --------- |
| 1.3.x   | Yes       |

## Reporting a Vulnerability

We use GitHub's private vulnerability reporting system. To report a security issue, please follow these steps:

1. Go to https://github.com/Rightbracket/peeroxide/security/advisories
2. Click "New draft advisory"
3. Fill in the details of the vulnerability

We will acknowledge your report and aim to provide a resolution within 90 days.

## Scope

Security issues include but are not limited to:
- Cryptographic flaws in our implementation of Noise XX, ChaCha20-Poly1305, Ed25519, or BLAKE2b.
- Authentication bypass or impersonation in P2P handshakes.
- Vulnerabilities in the Kademlia DHT implementation.
- Memory safety issues in the UDP transport or protocol implementation.
- Remote denial of service (DoS) attacks.

## Security Practices

Peeroxide implements the Hyperswarm P2P networking stack. We rely on standard cryptographic primitives and aim for high memory safety standards. This workspace includes libudx, peeroxide-dht, and the core peeroxide crate.

## License

This document is dual-licensed under MIT and Apache-2.0.
