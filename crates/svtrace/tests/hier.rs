//! The hierarchy (`slang.getScope`, `slang.getScopes`) on the fixtures: the shapes slang-server's view
//! reads, the names generate loops and instance arrays give their children, interface ports, types.

mod common;

use std::fs;

use serde_json::Value;
use svtrace::hier::{self, Hier};

fn hier(name: &str) -> Hier {
    let dir = common::data_dir();
    let dump = fs::read_to_string(dir.join(format!("{name}.json"))).expect("the fixture's dump");
    Hier::read(&dir, &svtrace::elaborate::parse(dump.as_bytes()).expect("valid JSON"))
}

fn names(items: &[Value]) -> Vec<String> {
    items.iter().map(|item| item["instName"].as_str().unwrap().to_string()).collect()
}

fn find<'a>(items: &'a [Value], name: &str) -> &'a Value {
    items.iter().find(|item| item["instName"] == name).unwrap_or_else(|| panic!("{name} in {:?}", names(items)))
}

fn children(item: &Value) -> &[Value] {
    item["children"].as_array().unwrap()
}

#[test]
fn roots_have_the_top_filled_and_the_packages_after() {
    let hier = hier("paths");
    let roots = hier.get_scope("");
    assert_eq!(names(&roots), ["top", "pkg"]);
    assert_eq!(roots[0]["kind"], "Instance");
    assert_eq!(roots[0]["declName"], "top");
    assert_eq!(roots[0]["declKind"], "Module");
    assert!(!children(&roots[0]).is_empty());
    assert_eq!(roots[1]["kind"], "Package");
    assert_eq!(roots[1]["declKind"], "Package");
    assert_eq!(roots[1]["hasChildren"], false);
    assert!(children(&roots[1]).is_empty());
    // The package's own members come with getScope.
    assert_eq!(names(&hier.get_scope("pkg")), ["pkg_sig"]);
    assert_eq!(hier.get_scope("pkg")[0]["type"], "logic");
}

#[test]
fn generate_loops_are_named_by_the_loop_variable() {
    let hier = hier("paths");
    let top = hier.get_scope("top");
    let gen_x = find(&top, "gen_x");
    assert_eq!(gen_x["kind"], "ScopeArray");
    assert_eq!(names(children(gen_x)), ["[2]", "[3]", "[4]"]);
    assert_eq!(children(gen_x)[0]["kind"], "Scope");
    // (The loop variable is the first member of an iteration, as a parameter.)
    assert_eq!(names(children(&children(gen_x)[1])), ["i", "v"]);
    assert_eq!(children(&children(gen_x)[1])[0]["value"], "3");
    assert_eq!(names(children(find(&top, "gen_neg"))), ["[-1]", "[0]"]);
    // Unnamed blocks take slang's names; the case branch that is not taken is left out.
    let all = names(&top);
    assert!(all.contains(&"genblk3".to_string()) && all.contains(&"case_one".to_string()));
    assert!(!all.contains(&"case_other".to_string()));
    // Nested loops, and the scopes on the way to something deep in them.
    let inner = find(children(&children(find(&top, "outer"))[1]), "inner_loop");
    assert_eq!(names(children(inner)), ["[0]", "[1]"]);
    let steps = hier.get_scopes("top.outer[1].inner_loop[0].u_in_gen.inner");
    let paths: Vec<&str> = steps.iter().map(|step| step["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        ["", "top", "top.outer", "top.outer[1]", "top.outer[1].inner_loop", "top.outer[1].inner_loop[0]", "top.outer[1].inner_loop[0].u_in_gen"]
    );
    assert_eq!(names(steps[0]["children"].as_array().unwrap()), ["top", "pkg"]);
}

#[test]
fn instance_arrays_are_listed_by_their_declared_range() {
    let hier = hier("paths");
    let top = hier.get_scope("top");
    let desc = find(&top, "u_desc");
    assert_eq!(desc["kind"], "InstanceArray");
    assert_eq!(desc["declName"], "leaf[3:0]");
    assert_eq!(desc["hasChildren"], true);
    assert_eq!(names(children(desc)), ["[3]", "[2]", "[1]", "[0]"]);
    assert_eq!(names(children(find(&top, "u_asc"))), ["[2]", "[3]", "[4]"]);
    assert_eq!(names(&hier.get_scope("top.u_asc")), ["[2]", "[3]", "[4]"]);
    // An element is an instance of its own, with the module's members and no children until asked.
    let element = &children(desc)[0];
    assert_eq!(element["kind"], "Instance");
    assert_eq!(element["declName"], "leaf");
    assert_eq!(names(&hier.get_scope("top.u_desc[1]")), ["d", "q", "inner"]);
    // A two-dimensional array is dropped from its parent, as the server does, but its rows can be asked for.
    assert!(!names(&top).contains(&"u_2d".to_string()));
    assert!(hier.get_scope("top.u_2d").is_empty());
    assert_eq!(names(&hier.get_scope("top.u_2d[1]")), ["[2]", "[3]"]);
    let steps = hier.get_scopes("top.u_desc[1].inner");
    let paths: Vec<&str> = steps.iter().map(|step| step["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["", "top", "top.u_desc", "top.u_desc[1]"]);
}

#[test]
fn values_have_the_server_kinds_and_types() {
    let hier = hier("paths");
    let top = hier.get_scope("top");
    let clk = find(&top, "clk");
    assert_eq!((clk["kind"].as_str(), clk["type"].as_str()), (Some("Port"), Some("input logic")));
    assert_eq!(find(&top, "q")["type"], "output logic");
    let p = find(&top, "P");
    assert_eq!((p["kind"].as_str(), p["type"].as_str(), p["value"].as_str()), (Some("Param"), Some("int"), Some("1")));
    assert_eq!(find(&top, "bus_d")["type"], "logic[3:0]");
    assert_eq!(find(&top, "bus_d")["kind"], "Logic");
    assert!(find(&top, "bus_d").get("children").is_none());
    // The location is the name's, in the file, from 0.
    let location = &clk["instLoc"];
    assert!(location["uri"].as_str().unwrap().ends_with("/paths.sv"));
    assert_eq!(location["range"]["start"]["line"], 11);
    assert_eq!(location["range"]["start"]["character"], 23);
    assert_eq!(location["range"]["end"]["character"], 26);
    // A declaration starts at its keyword.
    let top_item = &hier.get_scope("")[0];
    assert_eq!(top_item["declLoc"]["range"]["start"]["line"], 11);
    assert_eq!(top_item["declLoc"]["range"]["start"]["character"], 0);
}

#[test]
fn unknown_paths_have_no_scopes() {
    let hier = hier("paths");
    assert!(hier.get_scope("nope").is_empty());
    assert!(hier.get_scope("top.clk").is_empty());
    assert_eq!(hier.get_scopes("nope.x").len(), 1);
    assert_eq!(hier.get_scopes("").len(), 1);
    // A path that stops being one part way still gets the scopes up to there.
    let steps = hier.get_scopes("top.u_desc[1].missing");
    assert_eq!(steps.len(), 4);
    // A package is a step only when the path goes through it.
    assert_eq!(hier.get_scopes("pkg").len(), 1);
    let through = hier.get_scopes("pkg::pkg_sig");
    assert_eq!(through[1]["path"], "pkg");
}

#[test]
fn interface_ports_show_their_modport() {
    let hier = hier("cone_test");
    let sub = hier.get_scope("test.the_sub_1");
    let port = find(&sub, "qux_in");
    assert_eq!(port["kind"], "InterfacePort");
    assert_eq!(port["type"], "intf.quxIn");
    assert_eq!(names(children(port)), ["qux", "quz"]);
    assert_eq!(children(port)[0]["type"], "input logic[31:0]");
    assert_eq!(children(port)[0]["kind"], "Logic");
    let out = find(&sub, "qux_out");
    assert_eq!(children(out)[0]["type"], "output logic[31:0]");
    // The port is a scope of its own too.
    assert_eq!(names(&hier.get_scope("test.the_sub_1.qux_in")), ["qux", "quz"]);
    // An array of interface instances.
    let test = hier.get_scope("test");
    let intfs = find(&test, "the_intfs");
    assert_eq!(intfs["declName"], "intf[0:2]");
    assert_eq!(intfs["declKind"], "Interface");
    // Generate blocks without a name.
    assert!(names(&test).contains(&"genblk1".to_string()));
}

#[test]
fn types_are_spelled_as_the_server_does() {
    let hier = hier("hier");
    let top = hier.get_scope("hier_top");
    let type_of = |name: &str| find(&top, name)["type"].as_str().unwrap().to_string();
    assert_eq!(type_of("p_o"), "output PackedStruct pair_t (logic[4:0])");
    assert_eq!(type_of("count"), "int unsigned");
    assert_eq!(type_of("i32"), "integer");
    assert_eq!(type_of("tiny"), "shortint");
    assert_eq!(type_of("bytes"), "byte_t$[0:3]");
    assert_eq!(type_of("mem"), "logic[7:0]$[3:0][1:0]");
    assert_eq!(type_of("q"), "logic$[$]");
    assert_eq!(type_of("ratio"), "real");
    assert_eq!(type_of("name"), "string");
    assert_eq!(type_of("mode"), "Enum mode_e (logic[1:0])");
    assert_eq!(type_of("al"), "alias_t (aka logic[7:0])");
    assert!(type_of("state").starts_with("Enum enum{IDLE=32'sd0,RUN=32'sd1}e$"));
    assert!(type_of("state").ends_with(" (bit[31:0])"));
    assert_eq!(find(&top, "S")["value"], "\"hi\"");
    // A known one-bit constant is a digit.
    assert_eq!(find(&top, "ONE")["value"], "1");
    assert_eq!(find(&top, "ONE")["type"], "bit");
    // An uninstantiated block is left out; the loop's blocks are named by the loop variable.
    assert!(!names(&top).contains(&"never".to_string()));
    assert_eq!(names(children(find(&top, "gl"))), ["[1]", "[2]"]);
    // Parameters of a package: the values the way the server writes them.
    let package = hier.get_scope("hp");
    assert_eq!(find(&package, "DEPTH")["value"], "32'd4");
    assert_eq!(find(&package, "MODE")["type"], "Enum mode_e (logic[1:0])");
    assert_eq!(find(&package, "MODE")["value"], "2'b1");
}

#[test]
fn type_parameters_and_interface_ports_of_a_module() {
    let hier = hier("hier");
    let child = hier.get_scope("hier_top.u_child");
    let t = find(&child, "T");
    assert_eq!((t["kind"].as_str(), t["type"].as_str()), (Some("Param"), Some("type")));
    assert_eq!(t["value"], "byte_t (aka logic[7:0])");
    assert_eq!(find(&child, "W")["value"], "3");
    let port = find(&child, "port");
    assert_eq!(port["type"], "bus_if.dst");
    assert_eq!(names(children(port)), ["data", "valid"]);
    assert_eq!(children(port)[0]["type"], "input logic[7:0]");
    // The instance of an interface is an Instance of kind Interface.
    let top = hier.get_scope("hier_top");
    assert_eq!(find(&top, "bus")["declKind"], "Interface");
}

#[test]
fn the_command_takes_a_path() {
    let hier = hier("paths");
    let answer = hier::run(&hier, hier::GET_SCOPE, &[Value::String("top".into())]).unwrap();
    assert!(answer.as_array().unwrap().len() > 5);
    let steps = hier::run(&hier, hier::GET_SCOPES, &[Value::String(String::new())]).unwrap();
    assert_eq!(steps.as_array().unwrap().len(), 1);
    assert!(hier::run(&hier, hier::GET_SCOPE, &[]).is_err());
}

#[test]
fn an_escaped_name_is_reached_by_either_spelling() {
    // (edges.sv: `leaf \esc-inst (...)`. The view joins plain names; the trace hands over slang's path.)
    let hier = hier("edges");
    assert!(names(&hier.get_scope("top")).contains(&"esc-inst".to_string()));
    let plain = hier.get_scope("top.esc-inst");
    assert_eq!(names(&plain), ["x", "y"]);
    assert_eq!(hier.get_scope("top.\\esc-inst "), plain);
    let steps = hier.get_scopes("top.\\esc-inst .x");
    assert_eq!(steps.iter().map(|step| step["path"].as_str().unwrap()).collect::<Vec<_>>(), ["", "top", "top.esc-inst"]);
    assert_eq!(steps, hier.get_scopes("top.esc-inst.x"));
}

#[test]
fn every_location_in_the_fixtures_names_a_file() {
    // (A symbol without a place would get an empty URI, as the server gives it; none here has.)
    fn check(items: &[Value], seen: &mut usize) {
        for item in items {
            for key in ["instLoc", "declLoc"] {
                if let Some(location) = item.get(key) {
                    assert!(location["uri"].as_str().is_some_and(|uri| uri.starts_with("file:///")), "{item}");
                    *seen += 1;
                }
            }
            check(item["children"].as_array().map_or(&[][..], Vec::as_slice), seen);
        }
    }
    for name in ["paths", "hier", "edges", "cone_test"] {
        let hier = hier(name);
        let mut seen = 0;
        check(&hier.get_scope(""), &mut seen);
        assert!(seen > 0);
    }
}
