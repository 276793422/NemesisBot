//! Migration utilities for NemesisBot data.

pub mod config;
pub mod migrate;
pub mod openclaw_config;
pub mod workspace;

pub use config::{
    MigrateConfig, get_bool, get_bool_or_default, get_float, get_int, get_map, get_string,
    get_string_slice, hashmap_to_value, rewrite_workspace_path,
};
pub use migrate::{
    FullMigrationAction, FullMigrationActionType, FullMigrationResult, MigrateOptions, Migrator,
    confirm, execute, plan, print_plan, print_summary, run_full_migration,
};
pub use openclaw_config::{
    convert_config, find_openclaw_config, load_openclaw_config, merge_config,
};
pub use workspace::{MigrationPlan, MigrationResult, WorkspaceMigrator, migrate_workspace};
