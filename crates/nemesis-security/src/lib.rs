//! NemesisBot - Security Module
//!
//! 8-layer security pipeline:
//! 1. Injection Detection
//! 2. Command Guard
//! 3. ABAC (Auditor)
//! 4. Credential Scanner
//! 5. DLP (Data Loss Prevention)
//! 6. SSRF Guard
//! 7. Virus Scanner
//! 8. Audit Chain (Merkle integrity)
//!
//! Additional subsystems:
//! - Signature verification (Ed25519 / hash-based)
//! - Approval workflow for human authorisation
//! - Audit logging (file-persisted)
//! - Merkle tree for proof generation/verification
//! - ClamAV virus scanner integration

#[cfg(feature = "security")]
pub mod approval;
#[cfg(feature = "security")]
pub mod audit_log;
#[cfg(feature = "security")]
pub mod auditor;
#[cfg(feature = "security")]
pub mod clamav;
#[cfg(feature = "security")]
pub mod classifier;
#[cfg(feature = "security")]
pub mod command;
#[cfg(feature = "security")]
pub mod credential;
#[cfg(feature = "security")]
pub mod dlp;
#[cfg(feature = "security")]
pub mod guardian;
#[cfg(feature = "security")]
pub mod injection;
#[cfg(feature = "security")]
pub mod integrity;
#[cfg(feature = "security")]
pub mod matcher;
#[cfg(feature = "security")]
pub mod merkle;
#[cfg(feature = "security")]
pub mod middleware;
#[cfg(feature = "security")]
pub mod pipeline;
#[cfg(feature = "security")]
pub mod resolver;
#[cfg(feature = "security")]
pub mod scanner;
pub mod signature;
#[cfg(feature = "security")]
pub mod ssrf;
#[cfg(feature = "security")]
pub mod types;

#[cfg(feature = "security")]
pub use approval::{ApprovalManager, ApprovalRequest, ApprovalStatus, MultiProcessApprovalManager};
#[cfg(feature = "security")]
pub use audit_log::AuditLogger;
#[cfg(feature = "security")]
pub use auditor::SecurityAuditor;
#[cfg(feature = "security")]
pub use auditor::{AuditEvent, AuditFilter, AuditorConfig, OperationRequest};
#[cfg(feature = "security")]
pub use auditor::{
    DEFAULT_DENY_PATTERNS, get_audit_log, get_global_auditor, init_global_auditor,
    monitor_security_status,
};
#[cfg(feature = "security")]
pub use classifier::Classifier;
#[cfg(feature = "security")]
pub use matcher::{match_command_pattern, match_domain_pattern, match_pattern};
#[cfg(feature = "security")]
pub use merkle::MerkleTree;
#[cfg(feature = "security")]
pub use middleware::{
    BatchOperationRequest, DirEntry, FileMetadata, HttpRequest, HttpResponse, PermissionPreset,
    ProcessOutput, SecureFileWrapper, SecureHardwareWrapper, SecureNetworkWrapper,
    SecureProcessWrapper, SecurityMiddleware, create_agent_permission, create_cli_permission,
    create_web_permission,
};
#[cfg(feature = "security")]
pub use pipeline::SecurityPlugin;
#[cfg(feature = "security")]
pub use scanner::{
    DB_STATUS_MISSING, DB_STATUS_READY, DB_STATUS_STALE, DatabaseStatus, EngineInfo,
    ExtensionRules, INSTALL_STATUS_FAILED, INSTALL_STATUS_INSTALLED, INSTALL_STATUS_PENDING,
    ScanChain, ScanChainConfig, ScanChainResult, ScanEngine, ScanResult, ScannerEngineConfig,
    ScannerFullConfig, SharedScanChain, StubScanner, VirusScanner, available_engines,
    create_engine, shared_scan_chain,
};
pub use signature::{
    Config as SignatureConfig, KeyPair, SignatureVerifier, TrustLevel, TrustStore, TrustedKey,
    VerificationResult, Verifier, compute_fingerprint, compute_hash_signature, export_public_key,
    generate_key_pair, import_public_key, sign_content_hex, sign_file, sign_skill,
    verify_file_with_key, verify_signature_ed25519,
};
#[cfg(feature = "security")]
pub use types::*;
