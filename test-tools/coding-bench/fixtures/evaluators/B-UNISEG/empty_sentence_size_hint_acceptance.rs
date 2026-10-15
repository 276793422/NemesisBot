//! 隐藏验收：句子边界迭代器对空输入的 size_hint 不得下溢 panic，
//! 且必须满足 lower == 0、lower <= upper。
use unicode_segmentation::UnicodeSegmentation;

#[test]
fn sentence_iterators_size_hint_no_underflow_on_empty_input() {
    let (lower, upper) = "".split_sentence_bound_indices().size_hint();
    assert_eq!(lower, 0, "empty input lower bound must be 0, got {lower}");
    assert!(
        upper.is_none() || upper.unwrap() >= lower,
        "size_hint invariant violated: lower={lower} upper={upper:?}"
    );

    let (l2, u2) = "".split_sentence_bounds().size_hint();
    assert_eq!(l2, 0, "empty input lower bound must be 0, got {l2}");
    assert!(u2.is_none() || u2.unwrap() >= l2);

    let (l3, u3) = " ".split_sentence_bound_indices().size_hint();
    assert!(l3 <= u3.unwrap_or(usize::MAX));
}
