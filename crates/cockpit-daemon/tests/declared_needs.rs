//! #861: a provider whose sandbox declaration breaks the sandbox's rules is refused when the
//! daemon loads its providers: no session can name it, and readiness says why. The real
//! socket and routes, with fixture providers.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::path::PathBuf;
use std::sync::Arc;

use cockpit_client::Connection;
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{Command, Reply};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{
    Capabilities, InvalidValue, LaunchCommand, LaunchRequest, Needs, Provider, ProviderError,
    ProviderName,
};
use common::GENEROUS;

/// The fake provider, declaring that it reads `read`.
struct Declaring {
    inner: FakeProvider,
    read: &'static [&'static str],
}

impl Provider for Declaring {
    fn name(&self) -> &ProviderName {
        self.inner.name()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        self.inner.launch(request)
    }

    fn needs(&self) -> Result<Needs, InvalidValue> {
        Needs::new(&[], self.read, &[])
    }
}

fn declaring(root: &std::path::Path, name: &str, read: &'static [&'static str]) -> Declaring {
    Declaring {
        inner: FakeProvider::new(
            ProviderName::new(name).unwrap(),
            root.join(format!("{name}.log")),
        ),
        read,
    }
}

async fn start(root: &std::path::Path) -> Daemon {
    let wiring = common::wiring_in(
        root,
        vec![
            Arc::new(declaring(root, "fake", &["~/.local/share/fake"])),
            Arc::new(declaring(root, "keys", &["~/.ssh"])),
        ],
    );
    let options = Options {
        runtime_dir: root.join("run/cockpit"),
        state_dir: root.join("state/cockpit"),
        limits: Limits::default(),
    };
    let home: PathBuf = root.join("home");
    Instance::acquire(options)
        .unwrap()
        .start(wiring, |capabilities| {
            Arc::new(Routes::new(
                capabilities,
                cockpit_config::Environment {
                    home: Some(home.into()),
                    ..cockpit_config::Environment::default()
                },
            ))
        })
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provider_declaring_a_baseline_denied_path_is_not_listed() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = start(&dir.path().canonicalize().unwrap()).await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let reply = client
        .request(Command::ProviderList, GENEROUS)
        .await
        .unwrap();

    let Reply::Providers { providers } = reply else {
        panic!("{reply:?}");
    };
    let names: Vec<_> = providers.iter().map(|provider| &provider.name).collect();
    assert_eq!(names, ["fake"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readiness_names_why_a_providers_declaration_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = start(&dir.path().canonicalize().unwrap()).await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let reply = client
        .request(Command::Readiness { project: None }, GENEROUS)
        .await
        .unwrap();

    let Reply::Readiness { requirements } = reply else {
        panic!("{reply:?}");
    };
    let keys = requirements
        .iter()
        .find(|requirement| requirement.name == "provider keys")
        .unwrap_or_else(|| panic!("{requirements:?}"));
    assert!(
        keys.detail.contains("its sandbox declaration is refused")
            && keys.detail.contains("it declares ~/.ssh"),
        "{keys:?}"
    );
}
