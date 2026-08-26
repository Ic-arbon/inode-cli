use std::env;

fn main() {
    let reason = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: inode-routectl <reason>");
        std::process::exit(2);
    });
    if let Err(err) = inode_routectl::run(&reason) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
