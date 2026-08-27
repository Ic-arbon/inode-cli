use std::env;

fn main() {
    // libopenconnect's vpnc-script contract passes the reason in the `reason`
    // environment variable (the script is exec'd via `/bin/sh -c` with no
    // argv). Accept a positional argument as an override for direct use.
    let reason = env::args()
        .nth(1)
        .or_else(|| env::var("reason").ok())
        .unwrap_or_else(|| {
            eprintln!("usage: inode-routectl <reason>");
            std::process::exit(2);
        });
    if let Err(err) = inode_routectl::run(&reason) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
