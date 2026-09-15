extern crate std;

use crate::{
    SourceDatabase,
    data_plan::{self, DataPlanNodeKind, Format},
};
use alloc::{format, string::String, vec::Vec};

#[test]
fn data_parsing_and_deep_graphs_use_bounded_native_stack() {
    const PROBE: &str = "TELORA_DATA_STACK_PROBE";
    if std::env::var_os(PROBE).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "stack_tests::data_parsing_and_deep_graphs_use_bounded_native_stack",
                "--nocapture",
            ])
            .env(PROBE, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let nested = |n| format!("{}0{}", "[".repeat(n), "]".repeat(n));
            let wide = vec!["0"; 10000].join(",");
            let dotted = vec!["k"; 10000].join(".");
            let mut aliases = String::from("a0: &a0 [payload]\n");
            for i in 1..3000 {
                aliases.push_str(&format!("a{i}: &a{i} [*a{}]\n", i - 1));
            }
            let mut indented = String::new();
            for i in 0..100 {
                indented.push_str(&format!("{}k:\n", "  ".repeat(i)));
            }
            indented.push_str(&format!("{}0\n", "  ".repeat(100)));
            for (case, (format, source, valid)) in [
                (Format::Json, nested(32), true),
                (Format::Json, nested(33), false),
                (Format::Json, nested(2000), false),
                (Format::Json, format!("{}0", "[}".repeat(2000)), false),
                (Format::Json, format!("\"{}\"", "[".repeat(2000)), true),
                (Format::Toml, format!("a = {}", nested(32)), true),
                (Format::Toml, format!("a = {}", nested(2000)), false),
                (Format::Toml, format!("a = [{wide},]"), true),
                (Format::Toml, String::from("a = [1,,]"), false),
                (Format::Toml, format!("{dotted} = 1"), true),
                (Format::Toml, format!("a = {{ {dotted} = 1 }}"), true),
                (Format::Yaml, nested(24), true),
                (Format::Yaml, nested(2000), false),
                (Format::Yaml, indented, false),
                (Format::Yaml, format!("{}0", "&a ".repeat(2000)), false),
                (Format::Yaml, aliases, true),
            ]
            .into_iter()
            .enumerate()
            {
                std::eprintln!("data stack case {case}: {format:?}, {} bytes", source.len());
                let mut sources = SourceDatabase::default();
                let id = sources.add("stack-data", &source);
                let parsed = data_plan::parse_registered(&sources, id, format);
                std::eprintln!("parsed data stack case {case}");
                assert_eq!(
                    parsed.is_ok(),
                    valid,
                    "{format:?}: {:?}",
                    parsed.as_ref().err()
                );
                if let Ok(plan) = parsed {
                    let ordered = plan.into_postorder();
                    std::eprintln!("ordered data stack case {case}");
                    for (parent, node) in ordered.nodes().iter().enumerate() {
                        let children: Vec<_> = match &node.kind {
                            DataPlanNodeKind::Scalar(_) => Vec::new(),
                            DataPlanNodeKind::Array(items) => items.clone(),
                            DataPlanNodeKind::Object(fields) => {
                                fields.values().map(|f| f.value).collect()
                            }
                        };
                        assert!(children.iter().all(|id| id.index() < parent));
                    }
                } else {
                    assert!(parsed.unwrap_err().iter().any(|d| !d.labels.is_empty()));
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
