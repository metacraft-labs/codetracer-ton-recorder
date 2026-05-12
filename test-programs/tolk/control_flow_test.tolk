// Exercises Tolk's control-flow constructs: if/else, while, repeat,
// and do/until.  Spec-compliant recording would surface a step event
// per loop-body iteration and per branch arm, with the interleaved
// var-binding values reflecting real TVM execution.
//
// The current Tolk parser is line-oriented and only recognises
// var/val/return statements outside any nested block.  All four
// control-flow constructs below are therefore **invisible** to the
// recorder today (they're parsed neither as statements nor as
// expressions).  See RECORDER BUG notes in the matching test for the
// spec-compliant expectation.

fun classify(): int {
    var raw: int = 7;
    var sign: int = 0;
    if (raw < 0) {
        sign = -1;
    } else if (raw == 0) {
        sign = 0;
    } else {
        sign = 1;
    }
    return sign;
}

fun loop_sum(): int {
    var total: int = 0;
    var i: int = 0;
    while (i < 4) {
        total = total + i;
        i = i + 1;
    }
    return total;
}

fun repeat_count(): int {
    var counter: int = 0;
    repeat (3) {
        counter = counter + 1;
    }
    return counter;
}

fun do_until_grow(): int {
    var x: int = 1;
    do {
        x = x + x;
    } until (x >= 8);
    return x;
}

fun compute(): int {
    var sign: int = classify();
    var loop_total: int = loop_sum();
    var repeated: int = repeat_count();
    var grown: int = do_until_grow();
    var combined: int = sign + loop_total + repeated + grown;
    return combined;
}

fun main(): int {
    return compute();
}
