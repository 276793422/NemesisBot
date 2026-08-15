
use std::process::Command;
use std::os::windows::process::CommandExt;
fn main() {
    let tmpd = std::env::args().nth(1).unwrap();
    let agent = format!(r"{}\nemesisbot-eval-agent.exe", tmpd);
    let r = Command::new(r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\Start.exe")
        .arg("/box:NemesisEvalBox")
        .arg("/hide_window")
        .arg("/wait")
        .arg(&agent)
        .env("NEMESISBOT_ROLE", "eval-agent")
        .env("NEMESISBOT_EVAL_WORKSPACE", &tmpd)
        .env("NEMESISBOT_EVAL_PROMPT", "Create a file named hello.txt in the workspace with the content 'eval test'")
        .creation_flags(0x0800_0000)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match r {
        Ok(mut child) => {
            let st = child.wait();
            println!("spawn OK, wait={:?}", st.map(|s| s.code()));
        }
        Err(e) => println!("spawn ERR: {e}"),
    }
}
