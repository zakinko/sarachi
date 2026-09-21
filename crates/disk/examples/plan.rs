//! 書き込む前に配置を人間が確認するための出力。
//! 実ディスクには一切触れない。

use sarachi_disk::{Layout, RootKind};

fn main() -> anyhow::Result<()> {
    // root に何を載せるかで型 GUID が変わるので、そこも並べて見せる。
    for (label, bytes, sector, kind) in [
        (
            "Raspberry Pi / 32GB SD",
            32u64 << 30,
            512u64,
            RootKind::LinuxLuks,
        ),
        ("ノート / 512GB NVMe", 512 << 30, 4096, RootKind::LinuxLuks),
        ("FreeBSD / 64GB", 64 << 30, 512, RootKind::FreeBsdZfs),
        ("NetBSD / 64GB", 64 << 30, 512, RootKind::NetBsdCgd),
        ("下限ぎりぎり / 8GB", 8 << 30, 512, RootKind::LinuxLuks),
    ] {
        println!("== {label} ==");
        match Layout::plan(bytes, sector, kind) {
            Ok(l) => println!("{}", l.describe()),
            Err(e) => println!("  断った: {e}\n"),
        }
    }
    Ok(())
}
