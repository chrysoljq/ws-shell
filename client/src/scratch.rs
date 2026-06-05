use tokio::process::Command;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[tokio::main]
async fn main() {
    let mut c = std::process::Command::new("sh");
    c.process_group(0);
    let mut tc = Command::from(c);
}
