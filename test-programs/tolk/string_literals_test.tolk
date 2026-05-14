// Exercises Tolk's string-literal coverage: `"abc"` UTF-8 text
// literals, `0x"abcd"` hex slice literals, and the `;;` single-line
// comment marker (Tolk's FunC-derived alternative to `//`).
//
// Real-world TON contracts use these pervasively for op-code
// constants, content URIs, and prefix matching against incoming
// message bodies.  Pre-this-fixture the recorder dropped both
// literal flavours on the floor (the int-only TVM expression
// pipeline doesn't recognise them, and the recorder-side
// `try_eval_ton_call` only knew about `beginCell()` family calls).
//
// Spec-compliant recording surfaces each helper's slice binding as a
// real `ValueRecord::Raw` with a payload tag that distinguishes the
// two encodings: a hex literal MUST surface as `kind == "Raw"` whose
// `r:` payload starts with `Slice<hex>` and carries the parsed
// nibbles, while a text literal MUST surface with `r:` starting with
// `Slice<text>` and carrying the UTF-8 codepoints.  This lets a
// downstream consumer tell `0x"abcd"` apart from `"abcd"` even
// though both share the `Raw` ValueRecord wrapper.

;; This is a Tolk-style single-line comment (FunC double-semicolon).
;; The recorder must tolerate it and continue parsing.

fun build_text_slice() {
    var s = "abc";
    return s;
}

fun build_hex_slice() {
    var s = 0x"abcd";
    return s;
}

fun compute(): int {
    var t = build_text_slice();
    var h = build_hex_slice();
    return 0;
}

fun main(): int {
    return compute();
}
