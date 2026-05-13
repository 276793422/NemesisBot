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

pub mod pipeline;
pub mod auditor;
pub mod types;
pub mod middleware;
pub mod matcher;
pub mod injection;
pub mod command;
pub mod credential;
pub mod dlp;
pub mod ssrf;
pub mod integrity;
pub mod signature;
pub mod approval;
pub mod scanner;
pub mod audit_log;
pub mod merkle;
pub mod classifier;
pub mod resolver;
pub mod clamav;

pub use types::*;
pub use auditor::SecurityAuditor;
pub use auditor::{AuditorConfig, AuditEvent, AuditFilter, OperationRequest};
pub use auditor::{init_global_auditor, get_global_auditor, monitor_security_status, get_audit_log, DEFAULT_DENY_PATTERNS};
pub use pipeline::SecurityPlugin;
pub use matcher::{match_pattern, match_command_pattern, match_domain_pattern};
pub use signature::{
    SignatureVerifier, TrustStore, TrustLevel, Verifier, Config as SignatureConfig,
    VerificationResult, KeyPair, TrustedKey,
    generate_key_pair, export_public_key, import_public_key, compute_fingerprint,
    sign_file, sign_skill, sign_content_hex, verify_file_with_key,
    verify_signature_ed25519, compute_hash_signature,
};
pub use approval::{ApprovalManager, ApprovalRequest, ApprovalStatus, MultiProcessApprovalManager};
pub use scanner::{
    ScanEngine, ScanResult, ScanChainResult, StubScanner, VirusScanner,
    ScanChain, ScanChainConfig, ExtensionRules, EngineInfo, DatabaseStatus,
    ScannerEngineConfig, ScannerFullConfig, SharedScanChain, shared_scan_chain,
    create_engine, available_engines,
    INSTALL_STATUS_PENDING, INSTALL_STATUS_INSTALLED, INSTALL_STATUS_FAILED,
    DB_STATUS_MISSING, DB_STATUS_READY, DB_STATUS_STALE,
};
pub use audit_log::AuditLogger;
pub use merkle::MerkleTree;
pub use classifier::Classifier;
pub use middleware::{
    SecurityMiddleware, SecureFileWrapper, SecureProcessWrapper,
    SecureNetworkWrapper, SecureHardwareWrapper,
    PermissionPreset, BatchOperationRequest,
    FileMetadata, DirEntry, ProcessOutput,
    HttpResponse, HttpRequest,
    create_cli_permission, create_web_permission, create_agent_permission,
};
