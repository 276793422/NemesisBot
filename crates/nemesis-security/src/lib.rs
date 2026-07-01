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
pub mod pipeline;
#[cfg(feature = "security")]
pub mod auditor;
#[cfg(feature = "security")]
pub mod types;
#[cfg(feature = "security")]
pub mod middleware;
#[cfg(feature = "security")]
pub mod matcher;
#[cfg(feature = "security")]
pub mod injection;
#[cfg(feature = "security")]
pub mod command;
#[cfg(feature = "security")]
pub mod credential;
#[cfg(feature = "security")]
pub mod dlp;
#[cfg(feature = "security")]
pub mod ssrf;
#[cfg(feature = "security")]
pub mod integrity;
pub mod signature;
#[cfg(feature = "security")]
pub mod approval;
#[cfg(feature = "security")]
pub mod scanner;
#[cfg(feature = "security")]
pub mod audit_log;
#[cfg(feature = "security")]
pub mod merkle;
#[cfg(feature = "security")]
pub mod classifier;
#[cfg(feature = "security")]
pub mod resolver;
#[cfg(feature = "security")]
pub mod clamav;
#[cfg(feature = "security")]
pub mod guardian;

#[cfg(feature = "security")]
pub use types::*;
#[cfg(feature = "security")]
pub use auditor::SecurityAuditor;
#[cfg(feature = "security")]
pub use auditor::{AuditorConfig, AuditEvent, AuditFilter, OperationRequest};
#[cfg(feature = "security")]
pub use auditor::{init_global_auditor, get_global_auditor, monitor_security_status, get_audit_log, DEFAULT_DENY_PATTERNS};
#[cfg(feature = "security")]
pub use pipeline::SecurityPlugin;
#[cfg(feature = "security")]
pub use matcher::{match_pattern, match_command_pattern, match_domain_pattern};
pub use signature::{
    SignatureVerifier, TrustStore, TrustLevel, Verifier, Config as SignatureConfig,
    VerificationResult, KeyPair, TrustedKey,
    generate_key_pair, export_public_key, import_public_key, compute_fingerprint,
    sign_file, sign_skill, sign_content_hex, verify_file_with_key,
    verify_signature_ed25519, compute_hash_signature,
};
#[cfg(feature = "security")]
pub use approval::{ApprovalManager, ApprovalRequest, ApprovalStatus, MultiProcessApprovalManager};
#[cfg(feature = "security")]
pub use scanner::{
    ScanEngine, ScanResult, ScanChainResult, StubScanner, VirusScanner,
    ScanChain, ScanChainConfig, ExtensionRules, EngineInfo, DatabaseStatus,
    ScannerEngineConfig, ScannerFullConfig, SharedScanChain, shared_scan_chain,
    create_engine, available_engines,
    INSTALL_STATUS_PENDING, INSTALL_STATUS_INSTALLED, INSTALL_STATUS_FAILED,
    DB_STATUS_MISSING, DB_STATUS_READY, DB_STATUS_STALE,
};
#[cfg(feature = "security")]
pub use audit_log::AuditLogger;
#[cfg(feature = "security")]
pub use merkle::MerkleTree;
#[cfg(feature = "security")]
pub use classifier::Classifier;
#[cfg(feature = "security")]
pub use middleware::{
    SecurityMiddleware, SecureFileWrapper, SecureProcessWrapper,
    SecureNetworkWrapper, SecureHardwareWrapper,
    PermissionPreset, BatchOperationRequest,
    FileMetadata, DirEntry, ProcessOutput,
    HttpResponse, HttpRequest,
    create_cli_permission, create_web_permission, create_agent_permission,
};
