//! Migration utilities for NemesisBot data.

pub mod migrate;
pub mod config;
pub mod workspace;
pub mod openclaw_config;

pub use migrate::{
    Migrator, MigrateOptions, FullMigrationAction, FullMigrationActionType, FullMigrationResult,
    run_full_migration, plan, execute, confirm, print_plan, print_summary,
};
pub use workspace::{WorkspaceMigrator, MigrationPlan, MigrationResult, migrate_workspace};
pub use openclaw_config::{find_openclaw_config, load_openclaw_config, convert_config, merge_config};
pub use config::{
    MigrateConfig, get_map, get_string, get_float, get_int, get_bool, get_bool_or_default,
    get_string_slice, rewrite_workspace_path, hashmap_to_value,
};
