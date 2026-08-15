
use std::process::Command;
use std::os::windows::process::CommandExt;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let tmpd = &args[1];
    let agent = format!(r"{}\nemesisbot-eval-agent.exe", tmpd);
    let start = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\Start.exe";

    for round in 0..3 {
        // agent spawn (detached + output)
        let r = Command::new(start)
            .arg("/box:NemesisEvalBox")
            .arg("/hide_window")
            .arg(&agent)
            .env("NEMESISBOT_ROLE", "eval-agent")
            .env("NEMESISBOT_EVAL_WORKSPACE", tmpd)
            .env("NEMESISBOT_EVAL_PROMPT", "ping")
            .creation_flags(0x0000_0008)
            .output();
        match r {
            Ok(o) => println!("round {}: agent exit={:?}", round, o.status.code()),
            Err(e) => println!("round {}: err {e}", round),
        }
    }
}
