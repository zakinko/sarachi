//! 書き込む前に配置を人間が確認するための出力。
//! 実ディスクには一切触れない。

use sarachi_disk::Layout;

fn main() -> anyhow::Result<()> {
    for (label, bytes, sector) in [
        ("Raspberry Pi / 32GB SD", 32u64 << 30, 512u64),
        ("ノート / 512GB NVMe", 512 << 30, 4096),
        ("下限ぎりぎり / 8GB", 8 << 30, 512),
    ] {
        println!("== {label} ==");
        match Layout::plan(bytes, sector) {
            Ok(l) => println!("{}", l.describe()),
            Err(e) => println!("  断った: {e}\n"),
        }
    }
    Ok(())
}
