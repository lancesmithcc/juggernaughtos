Low-overhead post-quantum security stack
The goal is strong, quantum-resistant protection without slowing down the platform or adding unnecessary complexity.
Layer	Recommended technology	Purpose
Frontend	Leptos + Rust	Fast, interactive client portal
Backend	Axum + Tokio	Secure application server
Database	PostgreSQL + SQLx	Business data and tenant isolation
Network encryption	TLS 1.3 + X25519MLKEM768	Hybrid post-quantum connection security
Data encryption	AES-256-GCM	Fast encryption for private data
File encryption	AES-256-GCM	Protect images, documents, and videos
Key exchange	ML-KEM-768	Post-quantum protection when distributing encryption keys
Digital signatures	ML-DSA-65	Quantum-resistant signatures where needed
Key management	Vault or managed KMS	Secure key storage and rotation
Passwords	Argon2id	Secure password hashing
Authentication	Passkeys / WebAuthn	Phishing-resistant sign-in

Recommended Rust libraries
- rustls + aws-lc-rs: Hybrid post-quantum TLS, with X25519MLKEM768 supported by default in current configurations. 
  Rust
- aes-gcm: Authenticated AES-256 encryption for sensitive data.
- sqlx: PostgreSQL database access.
- argon2: Secure password hashing.
- secrecy: Reduce accidental exposure of secrets in Rust code.
Keep overhead low
1. Use AES-256-GCM for everyday data encryption, rather than post-quantum public-key algorithms on every file.
2. Use ML-KEM-768 for key establishment, not for encrypting large amounts of data directly.
3. Encrypt sensitive fields and private assets at the application layer.
4. Keep encryption keys outside PostgreSQL.
5. Use hybrid TLS with conventional fallback when clients lack support.