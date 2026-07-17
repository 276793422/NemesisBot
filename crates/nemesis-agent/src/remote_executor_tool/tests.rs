
    use super::*;

    #[test]
    fn build_command_no_start_exe_is_direct_spawn() {
        let ch = ExecutorChannel::new(
            PathBuf::from("/x/nemesisbot.exe"),
            "/ws".into(),
            Arc::new(|| false),
        );
        assert!(ch.start_exe.is_none());
        // No error — direct spawn command is built.
        let _ = ch.build_command();
    }

    #[test]
    fn build_command_with_start_exe_wraps() {
        let ch = ExecutorChannel::new(
            PathBuf::from("/x/nemesisbot.exe"),
            "/ws".into(),
            Arc::new(|| true),
        )
        .with_start_exe(PathBuf::from("/x/Start.exe"));
        assert!(ch.start_exe.is_some());
        // No error — Start.exe wrap command is built (L2.2 form).
        let _ = ch.build_command();
    }

    #[test]
    fn move_tools_is_the_expected_set() {
        assert_eq!(
            MOVE_TOOLS,
            &[
                "exec",
                "run_script",
                "read_file",
                "write_file",
                "list_dir",
                "edit_file",
                "append_file",
                "delete_file",
                "create_dir",
                "delete_dir",
                "grep",
                "git",
            ]
        );
    }
