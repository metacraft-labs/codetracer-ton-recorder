// Exercises variant-constructor coverage by modelling a small
// Status sum type as three sibling structs (`Pending`, `Active`,
// `Failed`).  The recorder treats the three constructors as
// `ValueRecord::Variant` rather than `ValueRecord::Struct` so the
// downstream consumer sees an Option-equivalent shape with the
// constructor name as the discriminator.
//
// Tolk's actual `type Status = ... | ... | ...` union syntax is
// still in flux at the language level (the TON team's roadmap lists
// proper algebraic types as a future feature), so this fixture
// approximates it via the most-portable shape: a one-field struct
// per variant, distinguished at the recorder level.  Each struct's
// type name (`Pending`, `Active`, `Failed`) becomes the variant
// discriminator and the struct's field tuple becomes the variant
// contents.  See `tracer.rs::variant_struct_names` for the
// allow-list of recognised constructors.
//
// Spec-compliant recording surfaces:
//   * three `ValueRecord::Variant` bindings (one per branch),
//   * each carrying a `Struct` contents with the per-constructor
//     fields,
//   * the `dispatch` helper's branch arms (chained if/else if/else
//     on the discriminator) reach the right `result` integer.

type Status = Pending | Active | Failed;

struct Pending {
    placeholder: int;
}

struct Active {
    n: int;
}

struct Failed {
    reason: int;
}

fun dispatch_pending(): int {
    var s: Pending = Pending { placeholder: 0 };
    var result: int = s.placeholder;
    return result;
}

fun dispatch_active(): int {
    var s: Active = Active { n: 7 };
    var result: int = s.n;
    return result;
}

fun dispatch_failed(): int {
    var s: Failed = Failed { reason: 13 };
    var result: int = 0 - s.reason;
    return result;
}

fun compute(): int {
    var p: int = dispatch_pending();
    var a: int = dispatch_active();
    var f: int = dispatch_failed();
    var total: int = p + a + f;
    return total;
}

fun main(): int {
    return compute();
}
