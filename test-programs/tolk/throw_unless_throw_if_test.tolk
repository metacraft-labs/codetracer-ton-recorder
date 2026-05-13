// Exercises Tolk's pervasive guard idiom: `throwIf(code, condition)`
// throws `code` when `condition` is truthy at runtime, and
// `throwUnless(code, condition)` throws `code` when `condition` is
// falsy at runtime.  Both are ubiquitous in real-world TON contracts
// (`throwUnless(36, msg::value() >= price);`, `throwIf(40, recipient
// == sender);`, ...).
//
// Pre-M10 the recorder only knew about the unconditional `throw <code>;`
// statement plus the static-sweep `assert(cond, code);` form.  Both
// were surfaced via the post-execution sweep
// (`emit_error_events_for_program`) — i.e. every syntactically present
// failure marker fired one io_event whether or not the path was
// reachable at runtime.  `throwIf` / `throwUnless` are different: they
// MUST evaluate the condition at runtime and only fire the io_event
// when the gate is met.  This fixture pins exactly that behaviour
// for the four canonical control-flow combinations:
//
//   probe_a = 5 -> guard_amount(5):    throwIf(40, 5 == 0)        -> NOP
//   probe_b = 0 -> guard_amount(0):    throwIf(40, 0 == 0)        -> ioError(40)
//   probe_c = 9 -> guard_threshold(9): throwUnless(36, 9 >= 7)    -> NOP
//   probe_d = 1 -> guard_threshold(1): throwUnless(36, 1 >= 7)    -> ioError(36)
//
// Each guard helper either returns 1 (gate satisfied) or aborts the
// caller via the matching io_event.

fun guard_amount(value: int): int {
    throwIf(40, value == 0);
    return 1;
}

fun guard_threshold(probe: int): int {
    throwUnless(36, probe >= 7);
    return 1;
}

fun compute(): int {
    var probe_a: int = 5;
    var ok_a: int = guard_amount(probe_a);
    var probe_b: int = 0;
    var ok_b: int = guard_amount(probe_b);
    var probe_c: int = 9;
    var ok_c: int = guard_threshold(probe_c);
    var probe_d: int = 1;
    var ok_d: int = guard_threshold(probe_d);
    var total: int = ok_a + ok_b + ok_c + ok_d;
    return total;
}

fun main(): int {
    return compute();
}
