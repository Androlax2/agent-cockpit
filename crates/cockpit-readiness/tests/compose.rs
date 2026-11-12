//! Whether a project's compose files run in parallel sessions on its one engine (#1371):
//! the scan names the line of each fixed name and literal host port, and readiness the
//! variable form that fixes it.

use cockpit_readiness::{ComposeFinding, ComposeProblem, scan};

fn literal(line: usize, port: &str) -> ComposeFinding {
    ComposeFinding {
        line,
        problem: ComposeProblem::LiteralPort {
            port: port.to_owned(),
        },
    }
}

#[test]
fn a_literal_host_port_is_found_on_its_line_in_every_syntax() {
    for (case, text, expected) in [
        (
            "short, quoted",
            "services:\n  web:\n    ports:\n      - \"8080:80\"\n",
            vec![literal(4, "8080")],
        ),
        (
            "short, bare",
            "services:\n  web:\n    ports:\n      - 8080:80\n",
            vec![literal(4, "8080")],
        ),
        (
            "with an address and a protocol",
            "services:\n  dns:\n    ports:\n      - '127.0.0.1:5353:53/udp'\n",
            vec![literal(4, "5353")],
        ),
        (
            "items at the key's indent",
            "services:\n  web:\n    ports:\n    - \"8080:80\"\n    image: nginx\n",
            vec![literal(4, "8080")],
        ),
        (
            "a range",
            "services:\n  web:\n    ports:\n      - \"9000-9001:9000-9001\"\n",
            vec![literal(4, "9000-9001")],
        ),
        (
            "flow",
            "services:\n  web:\n    ports: [\"8080:80\", \"8443:443\"]\n",
            vec![literal(3, "8080"), literal(3, "8443")],
        ),
        (
            "long",
            "services:\n  web:\n    ports:\n      - target: 80\n        published: 8080\n",
            vec![literal(5, "8080")],
        ),
        (
            "long, as a flow mapping",
            "services:\n  web:\n    ports:\n      - { target: 80, published: 8080 }\n",
            vec![literal(4, "8080")],
        ),
    ] {
        assert_eq!(scan(text), expected, "{case}");
    }
}

#[test]
fn a_port_from_a_variable_or_the_engine_is_not_reported() {
    for (case, text) in [
        (
            "variable",
            "services:\n  web:\n    ports:\n      - \"${TORLEK_PORT_0:-8080}:80\"\n",
        ),
        (
            "container port alone",
            "services:\n  web:\n    ports:\n      - \"80\"\n",
        ),
        (
            "long, variable",
            "services:\n  web:\n    ports:\n      - target: 80\n        published: \"${TORLEK_PORT_1}\"\n",
        ),
        (
            "commented out",
            "services:\n  web:\n    ports:\n      # - \"8080:80\"\n      - \"80\" # was 8080:80\n",
        ),
        (
            "expose",
            "services:\n  web:\n    expose:\n      - \"8080\"\n",
        ),
    ] {
        assert_eq!(scan(text), Vec::new(), "{case}");
    }
}

#[test]
fn a_fixed_container_name_is_found_and_one_from_a_variable_is_not() {
    let text = "services:\n  db:\n    container_name: app-db\n  cache:\n    container_name: ${COMPOSE_PROJECT_NAME}-cache\n";

    assert_eq!(
        scan(text),
        vec![ComposeFinding {
            line: 3,
            problem: ComposeProblem::ContainerName,
        }]
    );
}

#[test]
fn a_fixed_volume_or_network_name_is_found_and_a_named_entry_is_not() {
    let text = "services:\n  db:\n    image: postgres\n    volumes:\n      - data:/var/lib/data\n\
                volumes:\n  data:\n    name: app-data\n  name:\n    driver: local\n\
                networks:\n  back:\n    name: ${COMPOSE_PROJECT_NAME}-back\n  front:\n    name: app-front\n";

    assert_eq!(
        scan(text),
        vec![
            ComposeFinding {
                line: 8,
                problem: ComposeProblem::FixedName { kind: "volume" },
            },
            ComposeFinding {
                line: 15,
                problem: ComposeProblem::FixedName { kind: "network" },
            },
        ]
    );
}

#[test]
fn a_service_after_the_ports_list_is_not_read_as_a_port() {
    let text = "services:\n  web:\n    ports:\n      - \"80\"\n  db:\n    image: \"postgres:16\"\n";

    assert_eq!(scan(text), Vec::new());
}

#[test]
fn an_external_volume_or_network_or_a_nested_name_is_not_a_fixed_name() {
    for (case, text) in [
        (
            "external after name",
            "volumes:\n  shared:\n    name: team-cache\n    external: true\n",
        ),
        (
            "external before name",
            "networks:\n  proxy:\n    external: true\n    name: traefik\n",
        ),
        (
            "a driver option called name",
            "volumes:\n  data:\n    driver_opts:\n      name: tmpfs\n",
        ),
    ] {
        assert_eq!(scan(text), Vec::new(), "{case}");
    }
}

#[test]
fn a_fixed_name_is_found_on_the_last_entry_and_before_the_next_section() {
    let text = "volumes:\n  shared:\n    external: true\n  data:\n    name: app-data\n\
                networks:\n  back:\n    name: app-back\n";

    assert_eq!(
        scan(text),
        vec![
            ComposeFinding {
                line: 5,
                problem: ComposeProblem::FixedName { kind: "volume" },
            },
            ComposeFinding {
                line: 8,
                problem: ComposeProblem::FixedName { kind: "network" },
            },
        ]
    );
}
