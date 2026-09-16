//! 回復環境で走る実行体。
//!
//! WinPE 相当の中身にあたる。RAM 上の initramfs から走り、ディスクは
//! 一切マウントしていない状態で呼ばれることを前提にしている。
//!
//! **書き込みは既定で行わない。** `--commit` を明示しない限り、何を
//! するつもりかを表示して終わる。消去と再導入を扱う道具が、うっかり
//! 走って取り返しがつかなくなる余地を残すべきではない。

use anyhow::{Context, Result, bail};
use std::fs::OpenOptions;
use unix_mdm_disk::{Layout, write_gpt};

mod block;

fn usage() -> ! {
    eprintln!(
        "unix-mdm-recovery

  list                        見えているディスクを並べる
  plan <device>               その機体に対する配置を表示する（何も書かない）
  partition <device> --commit GPT を書く

引数を間違えた時に消えては困るので、--commit が無ければ書き込みはしない。"
    );
    std::process::exit(2)
}

fn find(name: &str) -> Result<block::Disk> {
    let disks = block::list()?;
    let key = name.trim_start_matches("/dev/");
    disks
        .into_iter()
        .find(|d| d.name == key)
        .with_context(|| format!("{name} というディスクが見つからない"))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else { usage() };

    match cmd {
        "list" => {
            let disks = block::list()?;
            if disks.is_empty() {
                println!("ディスクが見えない。ドライバが読み込まれているか確認すること");
            }
            for d in disks {
                println!("  {}", d.describe());
            }
        }

        "plan" => {
            let name = args.get(1).unwrap_or_else(|| usage());
            let d = find(name)?;
            println!("{}\n", d.describe());
            let layout = Layout::plan(d.size_bytes, d.logical_sector_size)?;
            print!("{}", layout.describe());
            println!("何も書いていない。書くなら partition ... --commit");
        }

        "partition" => {
            let name = args.get(1).unwrap_or_else(|| usage());
            let commit = args.iter().any(|a| a == "--commit");
            let d = find(name)?;
            let layout = Layout::plan(d.size_bytes, d.logical_sector_size)?;

            println!("{}\n", d.describe());
            print!("{}", layout.describe());

            if !commit {
                println!("dry-run。実際に書くには --commit を付けること");
                return Ok(());
            }

            // 取り外し可能な媒体は、作業者の USB である可能性がある。
            // 消すなとは言わないが、黙って消してよいものではない。
            if d.removable {
                bail!(
                    "{} は取り外し可能な媒体に見える。作業用の USB を消しかねないので断る",
                    d.name
                );
            }

            let mut f = OpenOptions::new()
                .write(true)
                .open(&d.path)
                .with_context(|| format!("{} を開けない", d.path.display()))?;
            write_gpt(&layout, &mut f)?;
            println!("\n{} に GPT を書いた", d.path.display());
        }

        _ => usage(),
    }
    Ok(())
}
