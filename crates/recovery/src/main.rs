//! 回復環境で走る実行体。
//!
//! WinPE 相当の中身にあたる。RAM 上の initramfs から走り、ディスクは
//! 一切マウントしていない状態で呼ばれることを前提にしている。
//!
//! **書き込みは既定で行わない。** `--commit` を明示しない限り、何を
//! するつもりかを表示して終わる。消去と再導入を扱う道具が、うっかり
//! 走って取り返しがつかなくなる余地を残すべきではない。

use anyhow::{Context, Result, bail};
use sarachi_disk::{Layout, RootKind, write_gpt};
use std::fs::OpenOptions;

mod block;
mod crypt;
mod fetch;
mod install;
mod provision;
mod wipe;

fn usage() -> ! {
    eprintln!(
        "sarachi-recovery

  list                        見えているディスクを並べる
  plan <device>               その機体に対する配置を表示する（何も書かない）
  partition <device> --commit GPT を書く
  install <device> --manifest <url> --image <url> --key <file> [--commit]
                              署名を検証しながら生のディスクへ書き戻す
                              --resume-from <n> で途中から
  wipe <device> --level reset|factory|destroy [--commit] [--root-kind <k>]
                              鍵を破棄して消す。段階は Windows の三段に対応
  provision <device> --base <url> --key <file> [--commit]
                     [--esp <name>] [--recovery <name>] [--rootfs <name>]
                     [--key-out <file>] [--print-key] [--root-kind <k>]
                              切る→ESPと回復領域を埋める→台ごとの鍵で
                              暗号化して root を書く、を一周

引数を間違えた時に消えては困るので、--commit が無ければ書き込みはしない。"
    );
    std::process::exit(2)
}

/// root に何を載せるかを読む。
///
/// 回復環境は常に Linux だが、導入する先は FreeBSD や NetBSD でありうるので、
/// 実行時に指定する。既定は Linux。
fn root_kind(args: &[String]) -> Result<RootKind> {
    let v = args
        .iter()
        .position(|a| a == "--root-kind")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("linux-luks");
    Ok(match v {
        "linux-luks" => RootKind::LinuxLuks,
        "freebsd-zfs" => RootKind::FreeBsdZfs,
        "freebsd-ufs" => RootKind::FreeBsdUfs,
        "netbsd-cgd" => RootKind::NetBsdCgd,
        "netbsd-ffs" => RootKind::NetBsdFfs,
        "openbsd-data" => RootKind::OpenBsdData,
        other => bail!(
            "--root-kind が分からない: {other}\n\
             linux-luks / freebsd-zfs / freebsd-ufs / netbsd-cgd / netbsd-ffs / openbsd-data"
        ),
    })
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
    let Some(cmd) = args.first().map(String::as_str) else {
        usage()
    };

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
            let layout = Layout::plan(d.size_bytes, d.logical_sector_size, root_kind(&args)?)?;
            print!("{}", layout.describe());
            println!("何も書いていない。書くなら partition ... --commit");
        }

        "partition" => {
            let name = args.get(1).unwrap_or_else(|| usage());
            let commit = args.iter().any(|a| a == "--commit");
            let d = find(name)?;
            let layout = Layout::plan(d.size_bytes, d.logical_sector_size, root_kind(&args)?)?;

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

        "install" => {
            let dev = args.get(1).unwrap_or_else(|| usage());
            let opt = |name: &str| -> Option<String> {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1))
                    .cloned()
            };
            let (Some(manifest_url), Some(image_url), Some(key_path)) =
                (opt("--manifest"), opt("--image"), opt("--key"))
            else {
                usage()
            };
            let trusted = install::load_key(std::path::Path::new(&key_path))?;
            let target = find(dev)?;
            let plan = install::Plan {
                manifest_url: &manifest_url,
                image_url: &image_url,
                target: &target.path,
                resume_from: opt("--resume-from")
                    .map(|v| v.parse())
                    .transpose()?
                    .unwrap_or(0),
                commit: args.iter().any(|a| a == "--commit"),
            };
            println!("{}\n", target.describe());
            install::run(&plan, &trusted)?;
        }

        "wipe" => {
            let dev = args.get(1).unwrap_or_else(|| usage());
            let level = match args
                .iter()
                .position(|a| a == "--level")
                .and_then(|i| args.get(i + 1))
                .map(String::as_str)
            {
                Some("reset") => sarachi_order::Level::Reset,
                Some("factory") => sarachi_order::Level::Factory,
                Some("destroy") => sarachi_order::Level::Destroy,
                _ => {
                    eprintln!("--level は reset / factory / destroy のいずれか");
                    std::process::exit(2);
                }
            };
            let d = find(dev)?;
            println!("{}\n", d.describe());
            if d.removable && args.iter().any(|a| a == "--commit") {
                bail!(
                    "{} は取り外し可能な媒体に見える。作業用の USB を消しかねないので断る",
                    d.name
                );
            }
            wipe::run(
                &d.path,
                d.size_bytes,
                d.logical_sector_size,
                root_kind(&args)?,
                level,
                args.iter().any(|a| a == "--commit"),
            )?;
        }

        "provision" => {
            let dev = args.get(1).unwrap_or_else(|| usage());
            let opt = |name: &str| -> Option<String> {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1))
                    .cloned()
            };
            let (Some(base), Some(key_path)) = (opt("--base"), opt("--key")) else {
                usage()
            };
            let rootfs = opt("--rootfs").unwrap_or_else(|| "rootfs".into());
            let esp = opt("--esp");
            let recovery = opt("--recovery");
            let key_out = opt("--key-out").unwrap_or_else(|| "/run/sarachi-root.key".into());
            let trusted = install::load_key(std::path::Path::new(&key_path))?;
            let d = find(dev)?;
            println!("{}\n", d.describe());
            if d.removable && args.iter().any(|a| a == "--commit") {
                bail!("{} は取り外し可能な媒体に見える。断る", d.name);
            }
            let plan = provision::Plan {
                disk: &d.path,
                size_bytes: d.size_bytes,
                sector_size: d.logical_sector_size,
                root_kind: root_kind(&args)?,
                bundle: provision::Bundle {
                    base: &base,
                    esp: esp.as_deref(),
                    recovery: recovery.as_deref(),
                    rootfs: &rootfs,
                },
                key_out: std::path::Path::new(&key_out),
                commit: args.iter().any(|a| a == "--commit"),
                print_key: args.iter().any(|a| a == "--print-key"),
            };
            provision::run(&plan, &trusted)?;
        }

        _ => usage(),
    }
    Ok(())
}
