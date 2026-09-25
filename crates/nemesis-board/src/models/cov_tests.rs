// models.rs 覆盖率补充测试（wave6：CommentType::as_str 的 Delivery 臂 100）。

use super::*;

/// CommentType::as_str 的 Delivery 臂（100）；顺带锁定全词表。
#[test]
fn w6_comment_type_as_str_delivery_arm() {
    assert_eq!(CommentType::Delivery.as_str(), "delivery");
    assert_eq!(CommentType::Comment.as_str(), "comment");
    assert_eq!(CommentType::StatusChange.as_str(), "status_change");
    assert_eq!(CommentType::System.as_str(), "system");
    assert_eq!(CommentType::Discussion.as_str(), "discussion");
    assert_eq!(CommentType::Question.as_str(), "question");
}
