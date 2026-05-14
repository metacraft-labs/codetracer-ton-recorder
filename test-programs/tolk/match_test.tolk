// Exercises Tolk's pattern-matching surface: `match (scrutinee) {
// Constructor => expr; Constructor(payload) => expr; ... }` over a
// user-defined sum type.  Pre-this-fixture the recorder dropped the
// `match` statement entirely (the line-oriented parser only knew
// about `var`/`val`/`return`/`throw`/`if`/`while`/`repeat`/`do`),
// so all three arms were silently invisible and the per-step trail
// stopped at the scrutinee binding.
//
// Spec-compliant recording surfaces:
//   * the scrutinee binding as a `ValueRecord::Variant` with the
//     construction-site discriminator,
//   * exactly one arm executes per dispatch — the one whose
//     constructor matches the scrutinee's discriminator (NOT a
//     lexical last-arm-wins fallback),
//   * payload bindings (`Active(n) => n`) bind the local name `n`
//     to the variant's first field for the duration of the arm.
//
// We exercise all three constructors via three top-level helpers so
// the trace pins each arm's behaviour independently.

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

fun classify(status: Status): int {
    var result: int = 0;
    match (status) {
        Pending => { result = 0; };
        Active(n) => { result = n; };
        Failed(reason) => { result = 0 - reason; };
    }
    return result;
}

fun dispatch_pending(): int {
    var s: Pending = Pending { placeholder: 99 };
    return classify(s);
}

fun dispatch_active(): int {
    var s: Active = Active { n: 7 };
    return classify(s);
}

fun dispatch_failed(): int {
    var s: Failed = Failed { reason: 13 };
    return classify(s);
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
