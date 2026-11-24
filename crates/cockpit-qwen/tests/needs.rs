//! What Qwen Code declares it needs from the sandbox and where its credentials are (#830,
//! #858), read through the provider contract.

use std::path::PathBuf;

use cockpit_provider_api::{CredentialHeader, HomePath, Provider};
use cockpit_qwen::QwenCode;

fn qwen() -> QwenCode {
    QwenCode::new(PathBuf::from("/usr/bin/qwen"), None, None)
}

fn strings<T: ToString>(values: &[T]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn it_reaches_qwen_oauth_s_model_api_and_token_refresh() {
    let hosts = strings(&qwen().needs().unwrap().hosts);

    for host in ["api.qwen.ai", "auth.qwen.ai", "chat.qwen.ai"] {
        assert!(hosts.iter().any(|listed| listed == host), "{host} missing");
    }
}

/// Telemetry fails closed, the catalog refresh is optional and the update check is off.
#[test]
fn it_reaches_no_telemetry_catalog_or_package_registry() {
    let hosts = strings(&qwen().needs().unwrap().hosts);

    for host in [
        "gb4w8c3ygj-default-sea.rum.aliyuncs.com",
        "models.dev",
        "registry.npmjs.org",
    ] {
        assert!(!hosts.iter().any(|listed| listed == host), "{host} listed");
    }
}

#[test]
fn it_reads_only_the_standalone_install_from_the_owner_s_home() {
    assert_eq!(
        strings(&qwen().needs().unwrap().read),
        ["~/.local/lib/qwen-code"]
    );
}

#[test]
fn its_settings_and_cache_are_written_into_the_session_s_own_folders() {
    assert_eq!(
        strings(&qwen().needs().unwrap().write),
        ["~/.qwen", "~/.cache/qwen-code"]
    );
}

#[test]
fn its_oauth_file_lies_in_a_folder_the_session_owns() {
    let needs = qwen().needs().unwrap();
    let credentials = qwen().credentials().unwrap();
    let oauth = HomePath::new("~/.qwen/oauth_creds.json").unwrap();

    assert_eq!(credentials.files, std::slice::from_ref(&oauth));
    assert!(needs.write.iter().any(|written| written.covers(&oauth)));
}

#[test]
fn each_preset_key_travels_as_bearer_to_its_own_hosts_only() {
    let credentials = qwen().credentials().unwrap();
    let bindings: Vec<(&str, CredentialHeader, Vec<String>)> = credentials
        .bindings
        .iter()
        .map(|binding| {
            (
                binding.variable.as_str(),
                binding.header,
                strings(&binding.hosts),
            )
        })
        .collect();

    let bearer = CredentialHeader::Bearer;
    assert_eq!(
        bindings,
        [
            (
                "DASHSCOPE_API_KEY",
                bearer,
                strings(&[
                    "dashscope.aliyuncs.com",
                    "dashscope-intl.aliyuncs.com",
                    "dashscope-us.aliyuncs.com",
                    "cn-hongkong.dashscope.aliyuncs.com",
                ])
            ),
            (
                "BAILIAN_CODING_PLAN_API_KEY",
                bearer,
                strings(&[
                    "coding.dashscope.aliyuncs.com",
                    "coding-intl.dashscope.aliyuncs.com"
                ])
            ),
            (
                "BAILIAN_TOKEN_PLAN_API_KEY",
                bearer,
                strings(&[
                    "token-plan.cn-beijing.maas.aliyuncs.com",
                    "token-plan.ap-southeast-1.maas.aliyuncs.com",
                ])
            ),
            (
                "QWENCLOUD_API_KEY",
                bearer,
                strings(&["maas.qianwenaiapi.com", "maas.qwencloudapi.com"])
            ),
            (
                "QWENCLOUD_TOKEN_PLAN_API_KEY",
                bearer,
                strings(&[
                    "token-plan.maas.qianwenaiapi.com",
                    "token-plan.maas.qwencloudapi.com"
                ])
            ),
        ]
    );
}

#[test]
fn it_offers_no_login_account() {
    assert_eq!(qwen().login().unwrap(), None);
}
