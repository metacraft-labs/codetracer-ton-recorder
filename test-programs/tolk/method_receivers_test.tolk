// Exercises Tolk's method-receiver syntax: `fun (self T) name()` —
// a Tolk-native idiom for attaching methods to a struct type so call
// sites can write `bag.length()` instead of `length(bag)`.  This is
// the canonical OO-flavoured surface that real Tolk contracts use
// for collection types (vectors, maps, queues) and for any helper
// that logically belongs to a particular struct.
//
// Pre-this-fixture the parser only recognised `fun NAME(params)`;
// the receiver-bearing `fun (self Bag) length()` declaration was
// either dropped or mis-parsed (the empty function name caused the
// declaration to be skipped).  Spec-compliant recording: each
// receiver-bearing helper registers under a qualified name
// `Bag.length` / `Bag.head`; method calls `bag.length()` resolve via
// the receiver's type to the qualified name and produce normal
// Call/Return events with `self` bound to the caller's struct.

struct Bag {
    a: int;
    b: int;
    c: int;
}

fun (self Bag) length(): int {
    return 3;
}

fun (self Bag) head(): int {
    return self.a;
}

fun compute(): int {
    var bag: Bag = Bag { a: 10, b: 20, c: 30 };
    var n: int = bag.length();
    var h: int = bag.head();
    var total: int = n + h;
    return total;
}

fun main(): int {
    return compute();
}
