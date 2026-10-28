//! End-to-end formatting microbench: import/compile/initialize/export outside timing.
use super::*;
use crate::{gc::GcConfig, session::Session, transport::Value};

#[test]
#[ignore = "explicit text writer performance experiment"]
fn measure_text_writer_paths() {
    let mut rows = vec![];
    for (case, input_type, expression) in [
        (
            "write_int",
            "Int",
            "s::join(a::map(xs,fn(x){fmt::render(fmt::write_int(fmt::empty(),x))}),\"\")",
        ),
        (
            "write_chain",
            "Int",
            "s::join(a::map(xs,fn(x){fmt::render(fmt::write_str(fmt::write_int(fmt::write_str(fmt::empty(),\"value=\"),x),\";\"))}),\"\")",
        ),
        (
            "to_string",
            "Int",
            "s::join(a::map(xs,fmt::to_string),\"\")",
        ),
        ("json_int", "Int", "json::stringify(codec::encode(xs))"),
        (
            "json_pretty",
            "Int",
            "json::stringify_pretty(2)(codec::encode(xs))",
        ),
        (
            "json_string",
            "String",
            "json::stringify(codec::encode(xs))",
        ),
        (
            "render_int",
            "Int",
            "s::join(a::map(xs,fn(x){fmt::render(fmt::from_int(x))}),\"\")",
        ),
        (
            "fmt_int",
            "Int",
            "fmt::render(fmt::concat(a::push(a::map(xs,fn(x){\"\"}),\"\"),a::map(xs,fmt::from_int)))",
        ),
        (
            "interpolate_int",
            "Int",
            "s::join(a::map(xs,fn(x){`value=\\{x};`}),\"\")",
        ),
    ] {
        if std::env::var("TELORA_TEXT_WRITER_CASES")
            .ok()
            .is_some_and(|names| !names.split(',').any(|name| name == case))
        {
            continue;
        }
        let source = format!(
            "use std::array as a; use std::string as s; use std::fmt as fmt; use std::json as json; use std::codec as codec; pub def answer: Fn(Array({input_type})) -> String = fn(xs) {{ {expression} }};"
        );
        let bytes = compile(&source).unwrap();
        for count in [128, 2048, 8192] {
            if std::env::var("TELORA_TEXT_WRITER_COUNTS")
                .ok()
                .is_some_and(|counts| !counts.split(',').any(|n| n == count.to_string()))
            {
                continue;
            }
            let input = if input_type == "String" {
                serde_json::json!(
                    (0..count)
                        .map(|i| format!(
                            "row-{i}-é🦀-\"\\\n\t-{}",
                            "abcdefghijklmnopqrstuvwxyz".repeat(3)
                        ))
                        .collect::<Vec<_>>()
                )
            } else {
                serde_json::json!((0..count).map(|i| i as i64 - 8192).collect::<Vec<_>>())
            };
            let expected = match case {
                "json_pretty" => serde_json::to_string_pretty(&input).unwrap(),
                "json_int" | "json_string" => serde_json::to_string(&input).unwrap(),
                "fmt_int" | "render_int" | "write_int" | "to_string" => input
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n.as_i64().unwrap().to_string())
                    .collect::<String>(),
                _ => input
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| format!("value={};", n.as_i64().unwrap()))
                    .collect::<String>(),
            };
            let mut samples = vec![];
            for _ in 0..3 {
                let mut session =
                    Session::load_with_limits(&bytes, 10_000_000_000, 256 * 1024 * 1024).unwrap();
                session
                    .configure_gc(GcConfig {
                        allocation_interval: 0,
                        heap_limit: 128 * 1024 * 1024,
                    })
                    .unwrap();
                session.initialize().unwrap();
                let entry = Value {
                    pointer: session.entry().unwrap(),
                    ty: session.manifest.entry_type,
                };
                let argument_type = session.manifest.types[entry.ty as usize].arguments[0];
                let input = session.input_value(argument_type, &input).unwrap();
                let before = session.gc_stats().unwrap().allocated_total;
                let fuel = session.store.get_fuel().unwrap();
                let start = std::time::Instant::now();
                let result = session.invoke_values(entry, &[input]).unwrap();
                let seconds = start.elapsed().as_secs_f64();
                let fuel = fuel - session.store.get_fuel().unwrap();
                let stats = session.gc_stats().unwrap();
                assert_eq!(
                    session.output_value(result).unwrap(),
                    serde_json::json!(expected)
                );
                let pin = session.pin_value(result).unwrap();
                session.collect_garbage().unwrap();
                let pinned = session.pinned_value(pin).unwrap();
                assert_eq!(
                    session.output_value(pinned).unwrap(),
                    serde_json::json!(expected)
                );
                samples.push(serde_json::json!({"seconds":seconds,"fuel":fuel,"managed_allocated_bytes":stats.allocated_total-before,"linear_memory_bytes":stats.memory_bytes}));
            }
            let row = serde_json::json!({"case":case,"count":count,"output_bytes":expected.len(),"artifact_bytes":bytes.len(),"samples":samples});
            println!("{row}");
            rows.push(row);
            if let Some(path) = std::env::var_os("TELORA_TEXT_WRITER_REPORT") {
                std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
            }
        }
    }
}
