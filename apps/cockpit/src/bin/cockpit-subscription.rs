use cockpit::{sessions, subscription_fallback};

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [command, id] = arguments.as_slice() else { return Err("usage: cockpit-subscription context <session> | gate <generation>".into()) };
    if command == "gate" {
        println!("{}", subscription_fallback::prompt_gate(id));
        return Ok(());
    }
    if command != "context" { return Err("unknown subscription command".into()); }
    let session = sessions::reported().into_iter().find(|session| session.session == *id).ok_or("no such session")?;
    println!("{}", subscription_fallback::handoff_context(&session)?);
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("cockpit-subscription: {error}");
        std::process::exit(1);
    }
}
