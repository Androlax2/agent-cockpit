//! Host-side native runtime and subscription evidence, collected before changing ownership.

use super::{Binding, Observation, Phase, RoutingStore, Selection, Stage};
use crate::{
    paths,
    sessions::Session,
    teams::{self, HostRequest},
    util,
};
use serde_json::{Value, json};
use std::process::Command;

pub fn source_receipt(id: &str) -> Result<Value, String> {
    teams::client::host_request(&HostRequest::SubscriptionSource { session: id.into() })
        .and_then(|reply| reply.result)
        .map_err(|error| error.to_string())
}

fn tmux(arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("timeout")
        .args(["2", "tmux"])
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() || output.stdout.len() > 65536 {
        return Err("native terminal inspection failed".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "native terminal returned invalid text".into())
}

pub fn input_boundary(session: &Session) -> Result<Option<String>, String> {
    if session.tmux_pane.len() < 2
        || !session.tmux_pane.starts_with('%')
        || !session.tmux_pane[1..]
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    {
        return Ok(None);
    }
    let position = tmux(&[
        "display-message",
        "-p",
        "-t",
        &session.tmux_pane,
        "#{cursor_x} #{cursor_y} #{pane_in_mode} #{session_name}",
    ])?;
    let fields: Vec<_> = position.split_whitespace().collect();
    let [column, row, "0", target] = fields.as_slice() else {
        return Ok(None);
    };
    let Ok(column) = column.parse::<u16>() else {
        return Ok(None);
    };
    if row.parse::<u16>().is_err()
        || !target.starts_with("claude-retry-")
        || !target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Ok(None);
    }
    if session.agent == "codex" && !target.starts_with("claude-retry-codex-") {
        return Ok(None);
    }
    let line = tmux(&[
        "capture-pane",
        "-p",
        "-e",
        "-t",
        &session.tmux_pane,
        "-S",
        row,
        "-E",
        row,
    ])?;
    Ok(super::native_input_is_clear(
        &session.agent,
        line.strip_suffix('\n').unwrap_or(&line),
        column,
    )
    .then(|| (*target).into()))
}

pub fn retire(origin: &str, generation: &str) -> Result<(), String> {
    let mut store = RoutingStore::load(&paths::subscription_routes())?;
    let route = store
        .route(origin)
        .cloned()
        .ok_or("no such subscription route")?;
    let Phase::Pending(pending) = route.phase else {
        return Err("subscription route is not pending".into());
    };
    if pending.generation != generation || pending.stage != Stage::Prepared {
        return Err("subscription retirement requires the prepared generation".into());
    }
    let check = |receipt: &Value| -> Result<Session, String> {
        let session =
            Session::from_value(receipt).ok_or("subscription source has no live runtime")?;
        let observed = Observation {
            binding: pending.source.clone(),
            session: session.clone(),
            current_capacity: crate::usage::SubscriptionCapacity::Unknown,
            original_capacity: crate::usage::SubscriptionCapacity::Unknown,
            alternate: None,
            has_pending_work: true,
            prompt_clear: true,
            manual_override: false,
        };
        if receipt["runtime"].as_str() != Some(&pending.source.runtime)
            || session.cwd != route.directory
            || session.repository != pending.source_repository
            || session.branch != pending.source_branch
            || !observed.safe_boundary()
        {
            return Err("subscription source changed or is no longer at a safe boundary".into());
        }
        Ok(session)
    };
    let receipt = source_receipt(&pending.source.session)?;
    if receipt.is_null() && pending.retirement_started {
        return store.mark_retired(origin, generation);
    }
    let session = match check(&receipt) {
        Ok(session) => session,
        Err(error) => {
            if pending.retirement_started
                && receipt["runtime"].as_str() == Some(&pending.source.runtime)
                && let Some(session) = Session::from_value(&receipt)
                && session.tmux_pane.starts_with('%')
                && session.tmux_pane[1..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
                && let Err(restore) = tmux(&["select-pane", "-e", "-t", &session.tmux_pane])
            {
                util::log_event(
                    "WARN",
                    &format!("subscription retirement recovery could not restore input: {restore}"),
                );
            }
            return Err(error);
        }
    };
    store.mark_retirement_started(origin, generation)?;
    tmux(&["select-pane", "-d", "-t", &session.tmux_pane])?;
    let result = (|| {
        let current = check(&source_receipt(&pending.source.session)?)?;
        if current.tmux_pane != session.tmux_pane {
            return Err("subscription source terminal changed".into());
        }
        let observed = inspect(&pending.source.session)?;
        if observed["binding"] != json!(pending.source) {
            return Err(
                "subscription source model, effort or execution account changed before retirement"
                    .into(),
            );
        }
        let target = input_boundary(&current)?
            .ok_or("subscription source has a draft or unconfirmed native input state")?;
        tmux(&["kill-session", "-t", &format!("={target}")])?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if source_receipt(&pending.source.session)?.is_null() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err("source retirement is unconfirmed; replacement stays paused".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        store.mark_retired(origin, generation)
    })();
    if result.is_err()
        && let Err(error) = tmux(&["select-pane", "-e", "-t", &session.tmux_pane])
    {
        util::log_event(
            "WARN",
            &format!("subscription source input could not be restored: {error}"),
        );
    }
    result
}

pub fn collect(agent: &str, directory: &str) -> Result<Value, String> {
    if !matches!(agent, "claude" | "codex") {
        return Err("unsupported native subscription".into());
    }
    let output = Command::new("python3")
        .arg(paths::native_subscription_script())
        .arg(agent)
        .current_dir(directory)
        .output()
        .map_err(|error| format!("cannot inspect native subscription: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{agent} subscription identity or quota is unconfirmed"
        ));
    }
    if output.stdout.len() > 1024 * 1024 {
        return Err("native subscription evidence exceeds 1 MiB".into());
    }
    let evidence: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "invalid native subscription evidence")?;
    let now = util::now() as i64;
    if evidence["fetched"]
        .as_i64()
        .is_none_or(|fetched| fetched > now || now - fetched > 600)
        || evidence["account"].as_str().is_none_or(|account| {
            account.len() != 64 || !account.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        || !evidence["usage"].is_object()
    {
        return Err("native subscription evidence is incomplete or stale".into());
    }
    Ok(evidence)
}

pub fn capacity(selection: &Selection, evidence: &Value) -> crate::usage::SubscriptionCapacity {
    crate::usage::subscription_capacity_of(
        &selection.provider,
        &selection.model,
        &evidence["usage"],
        evidence["fetched"].as_i64().unwrap_or(0),
        None,
        util::now() as i64,
        evidence["account"].as_str() == Some(&selection.account),
    )
}

pub fn candidate(agent: &str, directory: &str) -> Result<Selection, String> {
    let evidence = collect(agent, directory)?;
    candidate_from_evidence(agent, directory, &evidence)
}

fn candidate_from_evidence(
    agent: &str,
    directory: &str,
    evidence: &Value,
) -> Result<Selection, String> {
    let (model, variant) = if agent == "codex" {
        let model = evidence["models"]
            .as_array()
            .and_then(|models| models.iter().find(|model| model["isDefault"] == true))
            .ok_or("Codex has no verified default model")?;
        (
            model["model"]
                .as_str()
                .ok_or("invalid Codex default model")?
                .to_string(),
            model["defaultReasoningEffort"]
                .as_str()
                .ok_or("invalid Codex default effort")?
                .to_string(),
        )
    } else if agent == "claude" {
        let output = Command::new("python3")
            .arg(paths::native_subscription_script())
            .arg("discover-claude")
            .current_dir(directory)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() || output.stdout.len() > 16384 {
            return Err("native Claude candidate model is unavailable".into());
        }
        let model: Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| "invalid Claude candidate metadata")?;
        (
            model["model"]
                .as_str()
                .ok_or("missing Claude candidate model")?
                .to_string(),
            model["effort"]
                .as_str()
                .ok_or("missing Claude candidate effort")?
                .to_string(),
        )
    } else {
        return Err("unsupported native candidate".into());
    };
    let selected = Selection {
        agent: agent.into(),
        provider: if agent == "claude" {
            "anthropic"
        } else {
            "openai"
        }
        .into(),
        model,
        variant: Some(variant),
        account: evidence["account"]
            .as_str()
            .ok_or("unconfirmed candidate account")?
            .into(),
    };
    if !selected.valid() {
        return Err("invalid native candidate selection".into());
    }
    Ok(selected)
}

pub fn observe(id: &str) -> Result<Value, String> {
    use crate::usage::SubscriptionCapacity::{Available, Exhausted, Unknown};
    let routes = super::read_routes(&paths::subscription_routes())?;
    let existing = routes.iter().find(|route| route.owns_session(id));
    if existing.is_some_and(|route| {
        route.current.session != id
            || route.awaiting_initial_admission()
            || !matches!(route.phase, Phase::Active)
    }) {
        return Ok(Value::Null);
    }
    let todos = crate::todo::TodoList::load_checked()?;
    let receipt = source_receipt(id)?;
    let team_task = receipt["team_task"].as_str();
    let tasks: Vec<String> = todos
        .items
        .iter()
        .filter(|item| {
            (item.session() == id || team_task == Some(item.id().as_str()))
                && item.status() == "doing"
        })
        .map(|item| item.id())
        .chain(
            todos
                .issues
                .iter()
                .filter(|(key, issue)| {
                    (issue["session"] == id || team_task == Some(key.as_str()))
                        && issue["status"] == "doing"
                })
                .map(|(key, _)| key.clone()),
        )
        .collect();
    let source = inspect(id)?;
    let task = if let Some(route) = existing {
        route.task.clone()
    } else if tasks.len() == 1 {
        tasks[0].clone()
    } else if tasks.is_empty() && source["session"]["quota_stopped"] == true {
        format!("native-session:{id}")
    } else {
        return Ok(Value::Null);
    };
    let binding: Binding =
        serde_json::from_value(source["binding"].clone()).map_err(|error| error.to_string())?;
    let session = Session::from_value(&source["session"]).ok_or("native source disappeared")?;
    let current_capacity = capacity(&binding.selection, &source["evidence"]);
    let original_capacity = match existing {
        Some(route) if route.original.selection != binding.selection => {
            match collect(&route.original.selection.agent, &session.cwd) {
                Ok(evidence) => capacity(&route.original.selection, &evidence),
                Err(error) => {
                    util::log_event("WARN", &error);
                    Unknown
                }
            }
        }
        _ => current_capacity,
    };
    let mut alternate = None;
    if current_capacity == Exhausted && original_capacity != Available {
        let agent = if binding.selection.agent == "claude" {
            "codex"
        } else {
            "claude"
        };
        match collect(agent, &session.cwd) {
            Ok(evidence) => match candidate_from_evidence(agent, &session.cwd, &evidence) {
                Ok(selection) => {
                    let available = capacity(&selection, &evidence);
                    alternate = Some((selection, available));
                }
                Err(error) => util::log_event("WARN", &error),
            },
            Err(error) => util::log_event("WARN", &error),
        }
    }
    let mut store = RoutingStore::load(&paths::subscription_routes())?;
    store.track(binding.clone(), &session.cwd, &task, util::now() as i64)?;
    let has_pending_work = if store.route(id).is_some_and(|route| route.is_session_task())
        || existing.is_some_and(|route| route.is_session_task())
    {
        source["session"]["quota_stopped"] == true || session.state == "working"
    } else {
        tasks.contains(&task)
    };
    let observed = Observation {
        binding,
        session,
        current_capacity,
        original_capacity,
        alternate,
        has_pending_work,
        prompt_clear: source["prompt_clear"] == true,
        manual_override: false,
    };
    let transitions = store.reconcile(std::slice::from_ref(&observed), util::now() as i64)?;
    let Some(route) = store
        .routes()
        .find(|route| route.current.session == id || route.original.session == id)
        .cloned()
    else {
        return Ok(json!(transitions));
    };
    let reason = if transitions.is_empty() && matches!(route.phase, Phase::Active) {
        super::waiting_reason(
            &observed,
            route.current.selection != route.original.selection,
        )
    } else {
        None
    };
    if store.set_waiting(&route.original.session, reason.as_deref())?
        && let Some(reason) = &reason
    {
        crate::audit::record(
            "subscription",
            "subscription.waiting",
            "paused",
            json!({"origin":route.original.session,
                "task":route.task, "agent":route.current.selection.agent, "reason":reason}),
        );
    }
    for transition in &transitions {
        crate::audit::record(
            "subscription",
            "subscription.switch_prepared",
            "ok",
            json!({"origin":transition.origin,
            "generation":transition.generation, "direction":format!("{:?}", transition.direction),
            "from_agent":transition.source.selection.agent, "from_model":transition.source.selection.model,
            "to_agent":transition.target.agent, "to_model":transition.target.model}),
        );
    }
    Ok(json!(transitions))
}

pub fn sweep() -> Result<(), String> {
    let todos = crate::todo::TodoList::load_checked()?;
    {
        let mut store = RoutingStore::load(&paths::subscription_routes())?;
        let cancelled: Vec<_> = store
            .routes()
            .filter(|route| {
                !route.is_session_task()
                    && !matches!(route.phase, Phase::Cancelled | Phase::Completed)
                    && !todos
                        .items
                        .iter()
                        .any(|item| item.id() == route.task && item.status() == "doing")
                    && !todos
                        .issues
                        .get(&route.task)
                        .is_some_and(|issue| issue["status"] == "doing")
            })
            .map(|route| route.original.session.clone())
            .collect();
        for origin in cancelled {
            store.cancel(&origin)?;
        }
    }
    for session in crate::sessions::reported().into_iter().filter(|session| {
        matches!(session.agent.as_str(), "claude" | "codex")
            && !session.terminal
            && !session.credits
            && !session.is_external()
            && session.asleep.is_none()
    }) {
        if let Err(error) = observe(&session.session) {
            util::log_event(
                "WARN",
                &format!(
                    "subscription observation for {} paused: {error}",
                    session.session
                ),
            );
        }
    }
    for route in super::read_routes(&paths::subscription_routes())? {
        let Phase::Pending(pending) = route.phase else {
            continue;
        };
        let status = Command::new("cockpit")
            .args(["subscription-route", &pending.origin, &pending.generation])
            .status()
            .map_err(|error| format!("cannot dispatch subscription handoff: {error}"))?;
        if !status.success() {
            util::log_event(
                "WARN",
                &format!(
                    "subscription handoff {} remains pending",
                    pending.generation
                ),
            );
            let mut store = RoutingStore::load(&paths::subscription_routes())?;
            if store.set_waiting(
                &pending.origin,
                Some("the hand-off to the other subscription did not complete; it will retry"),
            )? {
                crate::audit::record(
                    "subscription",
                    "subscription.handoff_paused",
                    "paused",
                    json!({"origin":pending.origin,
                    "generation":pending.generation, "task":route.task, "stage":format!("{:?}", pending.stage)}),
                );
            }
        }
    }
    Ok(())
}

pub fn cancel_session(id: &str) -> Result<(), String> {
    if !crate::agents::is_session_id(id) {
        return Err("invalid subscription session identity".into());
    }
    let mut store = RoutingStore::load(&paths::subscription_routes())?;
    let origin = store
        .routes()
        .find(|route| route.owns_session(id))
        .map(|route| route.original.session.clone());
    if let Some(origin) = origin {
        store.cancel(&origin)?;
    }
    Ok(())
}

pub fn submit(origin: &str, generation: &str) -> Result<Value, String> {
    let route = RoutingStore::load(&paths::subscription_routes())?
        .route(origin)
        .cloned()
        .ok_or("no such subscription route")?;
    let Phase::Pending(pending) = route.phase else {
        return Err("subscription route is not pending".into());
    };
    if pending.generation != generation || pending.stage != Stage::Retired {
        return Err("native launch was already submitted or its source has not retired".into());
    }
    let evidence = collect(&pending.target.agent, &route.directory)?;
    if capacity(&pending.target, &evidence) != crate::usage::SubscriptionCapacity::Available {
        return Err("target included subscription capacity is no longer confirmed".into());
    }
    if pending.target.agent == "codex"
        && !evidence["models"].as_array().is_some_and(|models| {
            models.iter().any(|model| {
                model["model"].as_str() == Some(&pending.target.model)
                    && model["supportedReasoningEfforts"]
                        .as_array()
                        .is_some_and(|efforts| {
                            efforts.iter().any(|effort| {
                                effort["reasoningEffort"].as_str()
                                    == pending.target.variant.as_deref()
                            })
                        })
            })
        })
    {
        return Err("target Codex model and effort are unavailable".into());
    }
    let mut store = RoutingStore::load(&paths::subscription_routes())?;
    store.mark_submitted(origin, generation)?;
    Ok(json!({"agent":pending.target.agent,"directory":route.directory,"generation":generation}))
}

pub fn inspect(id: &str) -> Result<Value, String> {
    if !crate::agents::is_session_id(id) {
        return Err("invalid native session".into());
    }
    let receipt = source_receipt(id)?;
    let session =
        Session::from_value(&receipt).ok_or("native session has no live authenticated runtime")?;
    if session.terminal || session.credits || session.is_external() || session.asleep.is_some() {
        return Err("native session is not a managed subscription runtime".into());
    }
    let evidence = collect(&session.agent, &session.cwd)?;
    if receipt["subscription_account"].as_str().is_none()
        || receipt["subscription_account"] != evidence["account"]
    {
        return Err("native execution account is unconfirmed or differs from the quota account; restart through the updated cockpit launcher".into());
    }
    let (model, variant, native_thread, quota_stopped) = match session.agent.as_str() {
        "claude" => {
            let bytes = util::read_untrusted(
                &paths::statusline_dir().join(format!("statusline-{id}.json")),
                16385,
            )
            .map_err(|_| "native Claude model evidence is unavailable")?;
            if bytes.len() > 16384 {
                return Err("native Claude model evidence exceeds its bound".into());
            }
            let status: Value = serde_json::from_slice(&bytes)
                .map_err(|_| "invalid native Claude model evidence")?;
            if status["updated"].as_f64().is_none_or(|updated| {
                updated > util::now()
                    || session
                        .started
                        .is_none_or(|started| updated < started as f64)
            }) {
                return Err("native Claude model evidence belongs to an unconfirmed launch".into());
            }
            (
                status["model_id"].as_str().map(str::to_string),
                status["effort"].as_str().map(str::to_string),
                None,
                receipt["quota_stopped"] == true,
            )
        }
        "codex" => {
            let native = receipt["native_thread"]
                .as_str()
                .ok_or("native Codex thread is unavailable")?;
            if crate::codex::cockpit_id(native)? != id {
                return Err("native Codex thread identity mismatch".into());
            }
            let history = crate::codex::session_snapshot(native)?;
            if history["provider"] != "openai" {
                return Err("native Codex uses another provider".into());
            }
            (
                history["model"].as_str().map(str::to_string),
                history["effort"].as_str().map(str::to_string),
                Some(native.to_string()),
                history["quota_stopped"] == true,
            )
        }
        _ => return Err("unsupported native agent".into()),
    };
    let selection = Selection {
        agent: session.agent.clone(),
        provider: if session.agent == "claude" {
            "anthropic"
        } else {
            "openai"
        }
        .into(),
        model: model.ok_or("exact native model is unavailable")?,
        variant: Some(variant.ok_or("exact native effort is unavailable")?),
        account: evidence["account"].as_str().unwrap().into(),
    };
    let binding = Binding {
        session: id.into(),
        runtime: receipt["runtime"]
            .as_str()
            .ok_or("native runtime identity is unavailable")?
            .into(),
        selection,
        native_thread,
    };
    if !binding.valid() {
        return Err("invalid native runtime or model evidence".into());
    }
    let mut current = source_receipt(id)?;
    if current["runtime"] != receipt["runtime"]
        || current["cwd"] != receipt["cwd"]
        || current["agent"] != receipt["agent"]
    {
        return Err("native execution changed during observation".into());
    }
    let session = Session::from_value(&current).ok_or("native runtime disappeared")?;
    current["quota_stopped"] = json!(quota_stopped);
    let prompt_clear = input_boundary(&session)?.is_some();
    Ok(json!({"binding":binding,"session":current,"evidence":evidence,"prompt_clear":prompt_clear}))
}
