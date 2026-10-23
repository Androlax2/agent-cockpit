use cockpit::{sessions, subscription_fallback};

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let [command, origin, generation] = arguments.as_slice() {
        if command == "prepare-plan" {
            println!("{}", subscription_fallback::prepare_launch_plan(origin, generation)?.display());
            return Ok(());
        }
    }
    let [command, id] = arguments.as_slice() else { return Err("usage: cockpit-subscription context|message-target|queue-message <session> | gate <generation> | prepare-plan <origin> <generation>".into()) };
    if command == "gate" {
        println!("{}", subscription_fallback::prompt_gate(id));
        return Ok(());
    }
    if command == "message-target" {
        let (session, held) = subscription_fallback::message_target(id)?;
        println!("{}", serde_json::json!({"session":session,"held":held}));
        return Ok(());
    }
    if command == "queue-message" {
        use std::io::Read;
        let (session, _) = subscription_fallback::message_target(id)?;
        let mut bytes = Vec::new();
        std::io::stdin().take((cockpit::prompts::PROMPT_MAX * 4 + 1) as u64).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
        let message = std::str::from_utf8(&bytes).map_err(|_| "queued message is not valid UTF-8")?;
        cockpit::prompts::add(&session, message)?;
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
