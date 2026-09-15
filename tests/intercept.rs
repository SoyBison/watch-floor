//! Interception: what tree-sitter extraction actually records for a single
//! Python source file. Everything here goes through `common::sweep`, which
//! parses one string as module `pkg.mod` (file `pkg/mod.py`) with no git
//! involved, so each test pins down one extraction rule in isolation.

mod common;

use common::sweep;
use watch_floor::dossier::{Operation, Subject};
use watch_floor::intercept::{Interceptor, Sweep};

/// The class with this qualified name, or a failure naming what was found.
fn subject<'a>(swept: &'a Sweep, qualname: &str) -> &'a Subject {
    swept
        .subjects
        .iter()
        .find(|s| s.qualname == qualname)
        .unwrap_or_else(|| {
            panic!(
                "no class {qualname}; captured {:?}",
                qualnames(&swept.subjects)
            )
        })
}

/// The function with this callsign, or a failure naming what was found.
fn operation<'a>(swept: &'a Sweep, callsign: &str) -> &'a Operation {
    swept
        .operations
        .iter()
        .find(|o| o.callsign == callsign)
        .unwrap_or_else(|| {
            panic!(
                "no operation {callsign}; captured {:?}",
                callsigns(&swept.operations)
            )
        })
}

fn qualnames(subjects: &[Subject]) -> Vec<&str> {
    subjects.iter().map(|s| s.qualname.as_str()).collect()
}

fn callsigns(operations: &[Operation]) -> Vec<&str> {
    operations.iter().map(|o| o.callsign.as_str()).collect()
}

fn strs(items: &[String]) -> Vec<&str> {
    items.iter().map(String::as_str).collect()
}

// ---------------------------------------------------------------- classes ---

#[test]
fn class_records_qualname_bare_name_module_file_and_one_indexed_line() {
    let swept = sweep("# header\n\nclass Channel:\n    pass\n");

    assert_eq!(qualnames(&swept.subjects), ["pkg.mod.Channel"]);
    let channel = subject(&swept, "pkg.mod.Channel");
    assert_eq!(channel.name, "Channel");
    assert_eq!(channel.module, "pkg.mod");
    assert_eq!(channel.file, "pkg/mod.py");
    assert_eq!(
        channel.line, 3,
        "line is 1-indexed and points at the `class` keyword"
    );
}

#[test]
fn nested_class_is_qualified_under_its_outer_class() {
    let swept = sweep(concat!(
        "class Outer:\n",
        "    class Inner:\n",
        "        class Deepest:\n",
        "            def m(self): ...\n",
    ));

    assert_eq!(
        qualnames(&swept.subjects),
        [
            "pkg.mod.Outer",
            "pkg.mod.Outer.Inner",
            "pkg.mod.Outer.Inner.Deepest"
        ],
        "each nesting level prefixes the one below it"
    );
    assert_eq!(
        operation(&swept, "pkg.mod.Outer.Inner.Deepest.m")
            .owner
            .as_deref(),
        Some("pkg.mod.Outer.Inner.Deepest"),
        "the method belongs to the innermost class, not the outermost"
    );
}

#[test]
fn class_defined_inside_a_function_body_is_captured_under_that_function() {
    let swept = sweep(concat!(
        "def build():\n",
        "    class Local(Base):\n",
        "        def run(self): ...\n",
        "    return Local\n",
    ));

    let local = subject(&swept, "pkg.mod.build.Local");
    assert_eq!(local.name, "Local");
    assert_eq!(local.line, 2);
    assert_eq!(strs(&local.bases), ["Base"]);
    assert_eq!(strs(&local.methods), ["run"]);
    assert_eq!(
        operation(&swept, "pkg.mod.build.Local.run")
            .owner
            .as_deref(),
        Some("pkg.mod.build.Local"),
        "a function-local class still owns its methods"
    );
}

#[test]
fn decorated_class_line_points_at_the_class_keyword_not_the_decorator() {
    let swept = sweep("@register(\"x\")\n@final\nclass Deco:\n    pass\n");

    assert_eq!(subject(&swept, "pkg.mod.Deco").line, 3);
}

// -------------------------------------------------------- base normalizing ---

#[test]
fn plain_and_dotted_bases_are_recorded_as_written_in_declaration_order() {
    let swept = sweep("class Agent(abc.ABC, Channel, pkg.other.Mixin):\n    pass\n");

    assert_eq!(
        strs(&subject(&swept, "pkg.mod.Agent").bases),
        ["abc.ABC", "Channel", "pkg.other.Mixin"]
    );
}

#[test]
fn subscripted_base_records_only_the_generic_name() {
    let swept = sweep("class Box(Generic[T], Mapping[str, int]):\n    pass\n");

    assert_eq!(
        strs(&subject(&swept, "pkg.mod.Box").bases),
        ["Generic", "Mapping"],
        "the subscript describes type parameters, not inheritance"
    );
}

#[test]
fn keyword_arguments_and_splats_in_the_base_list_are_dropped() {
    let swept =
        sweep("class Meta(Base, metaclass=ABCMeta, total=False, *extra, **kw):\n    pass\n");

    assert_eq!(
        strs(&subject(&swept, "pkg.mod.Meta").bases),
        ["Base"],
        "only real base expressions survive"
    );
}

#[test]
fn dynamic_base_construction_records_the_callee_name() {
    let swept = sweep("class Model(make_base(\"users\"), namedtuple(\"P\", \"x\")):\n    pass\n");

    assert_eq!(
        strs(&subject(&swept, "pkg.mod.Model").bases),
        ["make_base", "namedtuple"]
    );
}

#[test]
fn class_with_no_base_list_has_no_bases() {
    let swept = sweep("class Bare:\n    pass\n\nclass Empty():\n    pass\n");

    assert!(subject(&swept, "pkg.mod.Bare").bases.is_empty());
    assert!(
        subject(&swept, "pkg.mod.Empty").bases.is_empty(),
        "`class Empty()` has an argument list, but nothing in it"
    );
}

// ---------------------------------------------------------------- methods ---

#[test]
fn class_methods_are_sorted_and_include_decorated_ones() {
    let swept = sweep(concat!(
        "class Handler:\n",
        "    def zeta(self): ...\n",
        "    @property\n",
        "    def alpha(self): ...\n",
        "    @staticmethod\n",
        "    async def middle(): ...\n",
    ));

    assert_eq!(
        strs(&subject(&swept, "pkg.mod.Handler").methods),
        ["alpha", "middle", "zeta"],
        "the method set is sorted so reordering a file is not a change"
    );
}

#[test]
fn nested_classes_and_assignments_are_not_listed_as_methods() {
    let swept = sweep(concat!(
        "class Handler:\n",
        "    limit = 3\n",
        "    class Config:\n",
        "        def load(self): ...\n",
        "    def assign(self): ...\n",
    ));

    assert_eq!(
        strs(&subject(&swept, "pkg.mod.Handler").methods),
        ["assign"],
        "only functions defined on the body itself count as methods"
    );
    assert_eq!(
        strs(&subject(&swept, "pkg.mod.Handler.Config").methods),
        ["load"]
    );
}

#[test]
fn class_methods_include_definitions_guarded_by_if_or_try() {
    let swept = sweep(concat!(
        "class Handler:\n",
        "    if TYPE_CHECKING:\n",
        "        def maybe(self): ...\n",
        "    try:\n",
        "        def risky(self): ...\n",
        "    except ImportError:\n",
        "        pass\n",
        "    def plain(self): ...\n",
    ));

    assert_eq!(
        strs(&subject(&swept, "pkg.mod.Handler").methods),
        ["maybe", "plain", "risky"],
        "these are captured as operations owned by Handler, so the method set must agree"
    );
}

#[test]
fn conditionally_defined_methods_are_still_captured_as_operations_of_the_class() {
    let swept = sweep(concat!(
        "class Handler:\n",
        "    if TYPE_CHECKING:\n",
        "        def maybe(self): ...\n",
        "    def plain(self): ...\n",
    ));

    assert_eq!(
        callsigns(&swept.operations),
        ["pkg.mod.Handler.maybe", "pkg.mod.Handler.plain"]
    );
    assert_eq!(
        operation(&swept, "pkg.mod.Handler.maybe").owner.as_deref(),
        Some("pkg.mod.Handler"),
        "a compound statement in the body does not change who owns the method"
    );
}

// -------------------------------------------------------------- functions ---

#[test]
fn function_records_callsign_line_and_no_owner_at_module_level() {
    let swept = sweep("\n\ndef transmit(payload):\n    pass\n");

    let transmit = operation(&swept, "pkg.mod.transmit");
    assert_eq!(transmit.name, "transmit");
    assert_eq!(transmit.module, "pkg.mod");
    assert_eq!(transmit.file, "pkg/mod.py");
    assert_eq!(transmit.line, 3);
    assert_eq!(transmit.owner, None);
    assert_eq!(strs(&transmit.params), ["payload"]);
}

#[test]
fn parameters_keep_source_order_and_splat_prefixes() {
    let swept = sweep("def f(a, b, *args, **kwargs):\n    pass\n");

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.f").params),
        ["a", "b", "*args", "**kwargs"]
    );
}

#[test]
fn parameter_annotations_and_defaults_are_ignored() {
    let swept =
        sweep("def f(a: int = 1, b = 2, c: str = \"x, y\", *args: int, **kw: Any):\n    pass\n");

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.f").params),
        ["a", "b", "c", "*args", "**kw"],
        "retyping or re-defaulting a parameter must not read as a signature change"
    );
}

#[test]
fn positional_only_and_keyword_only_separators_are_kept_in_place() {
    let swept = sweep("def f(p, /, q, *, r):\n    pass\n");

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.f").params),
        ["p", "/", "q", "*", "r"],
        "the separators are part of the calling convention, so they ride along"
    );
    assert_eq!(
        operation(&swept, "pkg.mod.f").signature(),
        "(p, /, q, *, r)"
    );
}

#[test]
fn function_with_no_parameters_has_an_empty_parameter_list() {
    let swept = sweep("def f():\n    pass\n");

    assert!(operation(&swept, "pkg.mod.f").params.is_empty());
}

// ------------------------------------------------------------- decorators ---

#[test]
fn decorators_attach_in_source_order_to_the_function_beneath_them() {
    let swept = sweep(concat!(
        "@app.route(\"/x\")\n",
        "@retry(times=3)\n",
        "@cached\n",
        "def handler():\n",
        "    pass\n",
    ));

    let handler = operation(&swept, "pkg.mod.handler");
    assert_eq!(
        strs(&handler.decorators),
        ["app.route(\"/x\")", "retry(times=3)", "cached"],
        "decorator expressions are kept as written, minus the @"
    );
    assert_eq!(
        handler.line, 4,
        "the line is the `def`, not the first decorator"
    );
}

#[test]
fn decorators_do_not_leak_onto_the_next_undecorated_definition() {
    let swept = sweep("@cached\ndef first():\n    pass\n\ndef second():\n    pass\n");

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.first").decorators),
        ["cached"]
    );
    assert!(
        operation(&swept, "pkg.mod.second").decorators.is_empty(),
        "a decorator applies to exactly one definition"
    );
}

#[test]
fn decorators_do_not_leak_into_a_decorated_functions_nested_definitions() {
    let swept = sweep("@cached\ndef outer():\n    def inner():\n        pass\n");

    assert!(operation(&swept, "pkg.mod.outer.inner")
        .decorators
        .is_empty());
}

// ------------------------------------------------------------------ async ---

#[test]
fn async_def_sets_is_async_and_plain_def_does_not() {
    let swept = sweep("async def go():\n    pass\n\ndef stay():\n    pass\n");

    assert!(operation(&swept, "pkg.mod.go").is_async);
    assert!(!operation(&swept, "pkg.mod.stay").is_async);
}

#[test]
fn decorated_async_method_is_still_marked_async() {
    let swept = sweep("class C:\n    @staticmethod\n    async def go():\n        pass\n");

    let go = operation(&swept, "pkg.mod.C.go");
    assert!(go.is_async, "the decorator must not hide the async keyword");
    assert_eq!(strs(&go.decorators), ["staticmethod"]);
}

// ------------------------------------------------------------------ calls ---

#[test]
fn identifier_and_attribute_callees_are_recorded_as_written() {
    let swept = sweep(concat!(
        "class Channel:\n",
        "    def send(self):\n",
        "        self.open()\n",
        "        a.b.c()\n",
        "        encode()\n",
    ));

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.Channel.send").raw_calls),
        ["self.open", "a.b.c", "encode"],
        "receivers are kept verbatim; resolution happens later, in the dossier"
    );
}

#[test]
fn calls_nested_inside_argument_lists_are_found_in_source_order() {
    let swept = sweep("def f():\n    wrap(inner(x), other.thing(deep()))\n");

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.f").raw_calls),
        ["wrap", "inner", "other.thing", "deep"],
        "the outer call is recorded first, then its arguments are descended into"
    );
}

#[test]
fn calls_inside_comprehensions_and_f_strings_belong_to_the_enclosing_function() {
    let swept =
        sweep("def f():\n    return [g(x) for x in h()]\n\ndef s():\n    return f\"{fmt(1)}\"\n");

    assert_eq!(strs(&operation(&swept, "pkg.mod.f").raw_calls), ["g", "h"]);
    assert_eq!(strs(&operation(&swept, "pkg.mod.s").raw_calls), ["fmt"]);
}

#[test]
fn super_call_records_the_attribute_chain_and_the_super_builtin_it_is_read_off() {
    let swept = sweep("class C(Base):\n    def m(self):\n        super().m()\n");

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.C.m").raw_calls),
        ["super().m", "super"],
        "`super().m` is what resolution keys off; the bare `super` is the builtin call itself"
    );
}

#[test]
fn callees_that_are_not_names_or_attribute_chains_are_skipped() {
    let swept = sweep(concat!(
        "def f():\n",
        "    handlers[0]()\n",
        "    (lambda: None)()\n",
        "    table[\"k\"](arg())\n",
    ));

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.f").raw_calls),
        ["arg"],
        "a subscripted or parenthesized callee cannot be resolved, but its arguments still are"
    );
}

#[test]
fn duplicate_call_targets_are_deduplicated_keeping_first_seen_order() {
    let swept = sweep(concat!(
        "def f():\n",
        "    b()\n",
        "    a()\n",
        "    b()\n",
        "    a.z()\n",
        "    a.z()\n",
        "    b()\n",
    ));

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.f").raw_calls),
        ["b", "a", "a.z"],
        "order is first appearance, not alphabetical, and repeats collapse"
    );
}

#[test]
fn calls_in_a_nested_def_belong_to_the_nested_def_not_the_enclosing_one() {
    let swept = sweep(concat!(
        "class C:\n",
        "    def outer(self):\n",
        "        first()\n",
        "        def inner():\n",
        "            only_inner()\n",
        "        @wrap\n",
        "        def decorated():\n",
        "            only_decorated()\n",
        "        last()\n",
    ));

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.C.outer").raw_calls),
        ["first", "last"],
        "neither the plain nor the decorated nested def leaks its calls upward"
    );
    assert_eq!(
        strs(&operation(&swept, "pkg.mod.C.outer.inner").raw_calls),
        ["only_inner"]
    );
    assert_eq!(
        strs(&operation(&swept, "pkg.mod.C.outer.decorated").raw_calls),
        ["only_decorated"]
    );
}

#[test]
fn calls_in_a_nested_class_belong_to_that_classs_methods() {
    let swept = sweep(concat!(
        "def outer():\n",
        "    before()\n",
        "    class Local:\n",
        "        def lm(self):\n",
        "            deep()\n",
        "    after()\n",
    ));

    assert_eq!(
        strs(&operation(&swept, "pkg.mod.outer").raw_calls),
        ["before", "after"]
    );
    assert_eq!(
        strs(&operation(&swept, "pkg.mod.outer.Local.lm").raw_calls),
        ["deep"]
    );
}

#[test]
fn module_level_code_is_not_attributed_to_any_operation() {
    let swept = sweep("configure()\nX = helper()\n\ndef f():\n    pass\n");

    assert_eq!(callsigns(&swept.operations), ["pkg.mod.f"]);
    assert!(
        operation(&swept, "pkg.mod.f").raw_calls.is_empty(),
        "import-time work belongs to no operation and is dropped"
    );
}

// ------------------------------------------------------------------ owner ---

#[test]
fn a_method_is_owned_by_its_class_but_a_function_nested_in_it_is_not() {
    let swept = sweep(concat!(
        "class Channel:\n",
        "    def send(self):\n",
        "        def encode():\n",
        "            pass\n",
    ));

    assert_eq!(
        operation(&swept, "pkg.mod.Channel.send").owner.as_deref(),
        Some("pkg.mod.Channel")
    );
    let encode = operation(&swept, "pkg.mod.Channel.send.encode");
    assert_eq!(
        encode.owner, None,
        "a closure is not a method of the class its enclosing method belongs to"
    );
    assert_eq!(encode.callsign, "pkg.mod.Channel.send.encode");
}

#[test]
fn module_and_file_are_stamped_on_every_definition_however_deeply_nested() {
    let mut interceptor = Interceptor::new().expect("interceptor");
    let swept = interceptor.sweep(
        "class Outer:\n    def m(self):\n        class Deep:\n            def d(self): ...\n",
        "cipher.agents",
        "src/cipher/agents.py",
    );

    assert_eq!(
        qualnames(&swept.subjects),
        ["cipher.agents.Outer", "cipher.agents.Outer.m.Deep"]
    );
    for s in &swept.subjects {
        assert_eq!(s.module, "cipher.agents");
        assert_eq!(s.file, "src/cipher/agents.py");
    }
    for o in &swept.operations {
        assert_eq!(o.module, "cipher.agents");
        assert_eq!(o.file, "src/cipher/agents.py");
    }
    assert_eq!(
        operation(&swept, "cipher.agents.Outer.m.Deep.d").display_name(),
        "Outer.m.Deep.d",
        "the module prefix is what gets stripped for display"
    );
}

// ------------------------------------------------------------- robustness ---

#[test]
fn a_file_with_a_syntax_error_still_yields_the_recovered_definitions() {
    let swept = sweep(concat!(
        "class Good:\n",
        "    def ok(self):\n",
        "        fine()\n",
        "\n",
        "def broken(:\n",
        "    pass\n",
        "\n",
        "def after():\n",
        "    tail()\n",
    ));

    assert_eq!(qualnames(&swept.subjects), ["pkg.mod.Good"]);
    assert_eq!(
        callsigns(&swept.operations),
        ["pkg.mod.Good.ok", "pkg.mod.broken", "pkg.mod.after"],
        "the half-written def does not cost us the definitions around it"
    );
    assert_eq!(
        strs(&operation(&swept, "pkg.mod.Good.ok").raw_calls),
        ["fine"]
    );
    assert_eq!(
        strs(&operation(&swept, "pkg.mod.after").raw_calls),
        ["tail"]
    );
}

#[test]
fn empty_and_comment_only_sources_yield_nothing_without_panicking() {
    for source in ["", "\n\n", "# nothing here\n", "   \t\n"] {
        let swept = sweep(source);
        assert!(
            swept.subjects.is_empty() && swept.operations.is_empty(),
            "expected no observations from {source:?}"
        );
    }
}
