use cockpit::{sessions, subscription_fallback};

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments == ["sweep"] {
        return subscription_fallback::native::sweep();
    }
    if let [command, agent, directory] = arguments.as_slice()
        && command == "candidate"
    {
        println!(
            "{}",
            serde_json::to_string(&subscription_fallback::native::candidate(agent, directory)?)
                .map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if let [command, provider, model] = arguments.as_slice()
        && command == "capacity"
    {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() > 1024 * 1024 {
            return Err("subscription evidence exceeds 1 MiB".into());
        }
        let evidence: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| "invalid subscription evidence")?;
        let capacity = cockpit::usage::subscription_capacity_of(
            provider,
            model,
            &evidence["usage"],
            evidence["fetched"]
                .as_i64()
                .ok_or("missing evidence timestamp")?,
            None,
            cockpit::util::now() as i64,
            true,
        );
        println!(
            "{}",
            match capacity {
                cockpit::usage::SubscriptionCapacity::Available => "available",
                cockpit::usage::SubscriptionCapacity::Exhausted => "exhausted",
                cockpit::usage::SubscriptionCapacity::Unknown => "unknown",
            }
        );
        return Ok(());
    }
    if let [command, origin, generation] = arguments.as_slice() {
        if command == "route-status" {
            use subscription_fallback::{Phase, RoutingStore};
            let store = RoutingStore::load(&cockpit::paths::subscription_routes())?;
            let route = store.route(origin).ok_or("no such subscription route")?;
            let stage = match &route.phase {
                Phase::Pending(pending) if pending.generation == *generation => {
                    format!("{:?}", pending.stage)
                }
                Phase::Active if route.admitted_generation.as_deref() == Some(generation) => {
                    "Active".into()
                }
                _ => return Err("subscription generation is no longer pending".into()),
            };
            println!("{}", serde_json::json!({"stage":stage}));
            return Ok(());
        }
        if command == "bind" {
            let request = cockpit::teams::HostRequest::SubscriptionBind {
                origin: origin.into(),
                generation: generation.into(),
            };
            let reply = cockpit::teams::client::host_request(&request)
                .and_then(|reply| reply.result)
                .map_err(|error| error.to_string())?;
            println!("{reply}");
            return Ok(());
        }
        if command == "submit" {
            println!(
                "{}",
                subscription_fallback::native::submit(origin, generation)?
            );
            return Ok(());
        }
        if command == "retire" {
            return subscription_fallback::native::retire(origin, generation);
        }
        if command == "prepare-plan" {
            println!(
                "{}",
                subscription_fallback::prepare_launch_plan(origin, generation)?.display()
            );
            return Ok(());
        }
    }
    let [command, id] = arguments.as_slice() else {
        return Err("usage: cockpit-subscription context|message-target|queue-message <session> | gate <generation> | prepare-plan <origin> <generation>".into());
    };
    if command == "gate" {
        println!("{}", subscription_fallback::prompt_gate(id));
        return Ok(());
    }
    if command == "start-gate" {
        println!("{}", subscription_fallback::initial_prompt_gate(id));
        return Ok(());
    }
    if command == "await-gate" {
        println!("{}", subscription_fallback::await_prompt_gate(id));
        return Ok(());
    }
    if command == "inspect" {
        println!("{}", subscription_fallback::native::inspect(id)?);
        return Ok(());
    }
    if command == "observe" {
        println!("{}", subscription_fallback::native::observe(id)?);
        return Ok(());
    }
    if command == "cancel-session" {
        return subscription_fallback::native::cancel_session(id);
    }
    if command == "message-target" {
        let (session, held) = subscription_fallback::message_target(id)?;
        let mut target = serde_json::json!({"session":session,"held":held});
        if subscription_fallback::has_native_inbox(&session)? {
            target["native_inbox"] = serde_json::json!(true);
        }
        println!("{target}");
        return Ok(());
    }
    if command == "queue-message" {
        use std::io::Read;
        let (session, _) = subscription_fallback::message_target(id)?;
        let mut bytes = Vec::new();
        std::io::stdin()
            .take((cockpit::prompts::PROMPT_MAX * 4 + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let message =
            std::str::from_utf8(&bytes).map_err(|_| "queued message is not valid UTF-8")?;
        cockpit::prompts::add(&session, message)?;
        return Ok(());
    }
    if command != "context" {
        return Err("unknown subscription command".into());
    }
    let session = sessions::reported()
        .into_iter()
        .find(|session| session.session == *id)
        .ok_or("no such session")?;
    println!("{}", subscription_fallback::handoff_context(&session)?);
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("cockpit-subscription: {error}");
        std::process::exit(1);
    }
}
