//! 配置を GPT としてイメージファイルへ書き出す。
//!
//! **イメージファイルにしか書かない。** 引数がブロックデバイスを指していたら
//! 断る。実ディスクへ書く経路は回復環境の側に置くべきもので、開発用の
//! 例題が実機を壊せてよい理由はない。

use anyhow::{Result, bail};
use std::fs::OpenOptions;
use unix_mdm_disk::{Layout, write_gpt};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "disk.img".into());
    let gib: u64 = args.next().unwrap_or_else(|| "32".into()).parse()?;
    let sector: u64 = args.next().unwrap_or_else(|| "512".into()).parse()?;

    if std::fs::metadata(&path).map(|m| !m.is_file()).unwrap_or(false) {
        bail!("{path} は通常ファイルではない。この例題はイメージにしか書かない");
    }

    let bytes = gib * 1024 * 1024 * 1024;
    let layout = Layout::plan(bytes, sector)?;
    print!("{}", layout.describe());

    let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(&path)?;
    f.set_len(bytes)?;
    write_gpt(&layout, &mut f)?;

    println!("{path} に書いた ({gib} GiB, {sector}B セクタ)");
    Ok(())
}
