//! 回復パーティションを含むディスク配置の定義と作成。
//!
//! 外部の `sgdisk` や `gpart` に頼らず自前で持つのは、回復環境が
//! OpenBSD や NetBSD を含む全標的で同じ実行体として動く必要があるため。
//! それらに `sgdisk` は無い。

mod crc32;
pub mod gpt;
pub mod layout;

pub use gpt::write_gpt;
pub use layout::{Layout, Partition, Role};
