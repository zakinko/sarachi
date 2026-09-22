//! 暗号層。
//!
//! ブロック層で暗号化し、鍵を破棄して消す、という一つの形で全標的を覆う。
//! 実装は OS ごとに違う（Linux は LUKS、FreeBSD と GhostBSD は geli、
//! NetBSD は cgd、OpenBSD は softraid crypto）が、上から見た振る舞いは同じに
//! なるよう trait にしてある。**消去の側のコードが OS ごとに分岐しない**のが
//! この形の一番の利点で、消去は設計全体の土台なので、そこを揃える価値がある。
//!
//! `open` が開いた先のパスを返すのは、それが OS ごとに違うため。LUKS は
//! `/dev/mapper/<name>`、geli は `<device>.eli`、cgd は `/dev/cgd<N>`、
//! OpenBSD は新しく現れる `/dev/sd<N>` になる。呼び出し側が組み立てられない。
//!
//! 鍵はここで作る。**イメージには一切入らない。** 配ったイメージに鍵が
//! 入っていると全台が同じマスター鍵を持ち、一台で鍵を破棄しても同じイメージを
//! 持つ者は誰でも復号できる。配ってしまってからでは取り返しがつかない。

use anyhow::{Context, Result, bail};
use sarachi_disk::RootKind;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 鍵の長さ。暗号層に渡す乱数で、マスター鍵そのものではない。
const KEY_BYTES: usize = 64;

pub trait Crypt {
    /// 記録と報告のための名前。
    fn name(&self) -> &'static str;

    /// 新しい鍵で容器を作る。**中身は失われる。**
    fn format(&self, device: &Path, key: &[u8]) -> Result<()>;

    /// 開いて、中身が現れたパスを返す。
    fn open(&self, device: &Path, key: &[u8], name: &str) -> Result<PathBuf>;

    /// 閉じる。`open` が返した path をそのまま渡す。
    ///
    /// 名前ではなく path を受けるのは、閉じるのに要る物が OS ごとに違うため。
    /// LUKS は `/dev/mapper/<name>` の name で閉じるが、geli は `.eli` を
    /// 外した元の provider で detach する。開いた側が返した物を渡せば、
    /// それぞれが自分に要る形を取り出せる。
    fn close(&self, mapped: &Path) -> Result<()>;

    /// crypto-erase。**これが消去の本体。**
    ///
    /// 上書きではなく鍵の破棄で消すのは、速いからではなく、SD と SSD では
    /// ウェアレベリングのせいで上書きが消去として成立しないため。
    /// Raspberry Pi を対象に含める以上、これ以外に手が無い。
    fn erase(&self, device: &Path) -> Result<()>;

    /// 自分の容器かどうか。消す前に確かめる。
    fn is_container(&self, device: &Path) -> bool;

    /// 鍵材料がディスク上にあるか。
    ///
    /// LUKS と geli は在る。ヘッダの中に鍵の入れ物を持っていて、そこを潰せば
    /// 中身は戻らない。**cgd には無い。** cgd は容器のヘッダを持たず、鍵は
    /// params の側にある。
    ///
    /// だから cgd の消去は「ここで壊す」にならない。鍵を手元に置かない形
    /// （起動のたびに control plane から取る）にしたうえで、消去は control
    /// plane で失効させることそのものになる。**黙って成功を返すわけには
    /// いかない**ので、呼ぶ側がこれを見て別の道を通る。
    fn has_on_disk_key(&self) -> bool {
        true
    }
}

/// root に載る物から暗号層を選ぶ。
///
/// 実装していない OS では、黙って別の手を使うのではなく断る。消去は
/// 取り返しがつかないので、確かめていない経路を通してはいけない。
/// 暗号層を組み立てるのに要る、台ごとの事情。
///
/// LUKS と geli は device と鍵だけで済むが、**cgd は済まない。** 鍵を手元に
/// 置かない形にした以上、「どこから取ってくるか」を params に書く必要があり、
/// それは台ごとに違う。
pub struct Setup<'a> {
    /// この台の名前。control plane で台を見分けるのに使う。
    pub device_id: &'a str,
    /// control plane の在処。無ければ手元のファイルから取る（試験用）。
    pub escrow_url: Option<&'a str>,
    /// cgd の params の置き場。root の外に置く。**鍵は入らない。**
    pub params: &'a Path,
}

impl Default for Setup<'_> {
    fn default() -> Self {
        Self {
            device_id: "unknown",
            escrow_url: None,
            params: Path::new("/etc/cgd/sarachi-root"),
        }
    }
}

pub fn for_kind(kind: RootKind, cx: &Setup) -> Result<Box<dyn Crypt>> {
    match kind {
        // DragonFly もここ。geli ではなく LUKS を持つことを実機で確かめた。
        RootKind::LinuxLuks | RootKind::DragonFlyLuks => Ok(Box::new(Luks)),
        RootKind::FreeBsdZfs | RootKind::FreeBsdUfs => Ok(Box::new(Geli)),
        RootKind::NetBsdCgd | RootKind::NetBsdFfs => {
            // 鍵は手元に置かない。開くたびにここから取る。
            let mut cmd = format!("sarachi-recovery key-fetch --device-id {}", cx.device_id);
            if let Some(u) = cx.escrow_url {
                cmd.push_str(&format!(" --escrow-url {u}"));
            }
            Ok(Box::new(Cgd::new(cmd, cx.params.to_path_buf())))
        }
        RootKind::OpenBsdData => {
            bail!("softraid crypto はまだ実装していない（OpenBSD）。実機で確かめるまで入れない")
        }
    }
}

/// 台ごとの鍵を作る。
///
/// `/dev/urandom` から取る。Linux の `urandom` は初期化後は `random` と同じ
/// 品質で、初期化前は読みが待たされるだけなので、`crng init done` を待つ必要は
/// ない。
pub fn generate_key() -> Result<Vec<u8>> {
    let mut key = vec![0u8; KEY_BYTES];
    std::fs::File::open("/dev/urandom")
        .context("/dev/urandom を開けない")?
        .read_exact(&mut key)?;
    Ok(key)
}

/// 外部の道具を一つ走らせる。失敗したら stderr を添えて返す。
///
/// LUKS と geli で同じ形なので、ここに一つ置いて両方から使う。
fn run(exe: &str, what: &str, args: &[&str], stdin: Option<&[u8]>) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let mut c = Command::new(exe);
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() {
        c.stdin(Stdio::piped());
    }
    let mut child = c
        .spawn()
        .with_context(|| format!("{exe} を起動できない（{what}）"))?;
    if let (Some(data), Some(mut s)) = (stdin, child.stdin.take()) {
        s.write_all(data)?;
        drop(s);
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "{what} に失敗（{}）\n  {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// 候補のうち最初に在るものを返す。無ければ名前だけ返して PATH に任せる。
///
/// 絶対パスで呼ぶのは、initramfs の PATH が当てにできないため。
/// `Command::new("cryptsetup")` は spawn 時に ENOENT を返し、その誤りは
/// 「道具が壊れている」とも「ライブラリが足りない」とも読めるので、
/// 原因にたどり着くのに手間がかかる。
fn first_existing(cands: &[&'static str], fallback: &'static str) -> &'static str {
    for p in cands {
        if Path::new(p).exists() {
            return p;
        }
    }
    fallback
}

// ---------------------------------------------------------------- LUKS

pub struct Luks;

impl Luks {
    fn exe() -> &'static str {
        first_existing(
            &[
                "/sbin/cryptsetup",
                "/usr/sbin/cryptsetup",
                "/bin/cryptsetup",
            ],
            "cryptsetup",
        )
    }
}

impl Crypt for Luks {
    fn name(&self) -> &'static str {
        "LUKS2"
    }

    fn format(&self, device: &Path, key: &[u8]) -> Result<()> {
        let dev = device.to_string_lossy().to_string();
        run(
            Self::exe(),
            "LUKS2 の作成",
            &[
                "luksFormat",
                "--type",
                "luks2",
                "--batch-mode",
                "--pbkdf",
                "argon2id",
                // 回復環境の RAM は有限で、Pi では 2GB しかない。既定のまま
                // argon2id を回すと空きメモリの半分を取りにいって落ちうる。
                "--pbkdf-memory",
                "65536",
                "--key-file",
                "-",
                &dev,
            ],
            Some(key),
        )
    }

    fn open(&self, device: &Path, key: &[u8], name: &str) -> Result<PathBuf> {
        let dev = device.to_string_lossy().to_string();
        run(
            Self::exe(),
            "LUKS の展開",
            &["open", "--key-file", "-", &dev, name],
            Some(key),
        )?;
        Ok(Path::new("/dev/mapper").join(name))
    }

    fn close(&self, mapped: &Path) -> Result<()> {
        let name = mapped
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        run(Self::exe(), "LUKS の閉鎖", &["close", &name], None)
    }

    fn erase(&self, device: &Path) -> Result<()> {
        let dev = device.to_string_lossy().to_string();
        run(
            Self::exe(),
            "crypto-erase",
            &["luksErase", "--batch-mode", &dev],
            None,
        )
    }

    fn is_container(&self, device: &Path) -> bool {
        let dev = device.to_string_lossy().to_string();
        Command::new(Self::exe())
            .args(["isLuks", &dev])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------- geli

/// FreeBSD と GhostBSD の geli。
///
/// **DragonFly はここへ来ない。** 以前は FreeBSD 系だからと geli に落として
/// いたが、実機で確かめたところ /sbin/geli は存在せず、base に
/// /sbin/cryptsetup と /sbin/dmsetup が在り、dm_target_crypt.ko が読み込めた。
/// DragonFly は `RootKind::DragonFlyLuks` から `Luks` を通す。
pub struct Geli;

impl Geli {
    fn exe() -> &'static str {
        first_existing(&["/sbin/geli", "/usr/sbin/geli"], "geli")
    }

    /// geli は `.eli` を足した provider を作る。元の provider に戻す。
    fn provider(mapped: &Path) -> PathBuf {
        let s = mapped.to_string_lossy();
        match s.strip_suffix(".eli") {
            Some(base) => PathBuf::from(base),
            None => mapped.to_path_buf(),
        }
    }
}

impl Crypt for Geli {
    fn name(&self) -> &'static str {
        "geli (AES-XTS 256)"
    }

    fn format(&self, device: &Path, key: &[u8]) -> Result<()> {
        let dev = device.to_string_lossy().to_string();
        run(
            Self::exe(),
            "geli の作成",
            &[
                "init",
                // 控えを作らせない。既定では metadata の控えが
                // /var/backups/<provider>.eli に落ちるが、`geli kill` は
                // それを消さない。控えが残っていると鍵を破棄しても復号
                // できてしまい、crypto-erase が消去として成立しなくなる。
                // 消去は設計の土台なので、ここは落としてはいけない。
                "-B", "none",
                // パスフレーズは持たせない。鍵は鍵ファイルだけ。
                "-P", "-K", "-", "-e", "AES-XTS", "-l", "256", "-s", "4096", &dev,
            ],
            Some(key),
        )
    }

    fn open(&self, device: &Path, key: &[u8], _name: &str) -> Result<PathBuf> {
        // name は使わない。geli は名前を選ばせず、常に <provider>.eli になる。
        let dev = device.to_string_lossy().to_string();
        run(
            Self::exe(),
            "geli の展開",
            &["attach", "-p", "-k", "-", &dev],
            Some(key),
        )?;
        Ok(PathBuf::from(format!("{dev}.eli")))
    }

    fn close(&self, mapped: &Path) -> Result<()> {
        let dev = Self::provider(mapped).to_string_lossy().to_string();
        run(Self::exe(), "geli の閉鎖", &["detach", &dev], None)
    }

    fn erase(&self, device: &Path) -> Result<()> {
        let dev = device.to_string_lossy().to_string();
        // kill は metadata の鍵を潰す。上書きではないので SD や SSD でも
        // 成立する。
        run(Self::exe(), "crypto-erase", &["kill", &dev], None)
    }

    fn is_container(&self, device: &Path) -> bool {
        let dev = device.to_string_lossy().to_string();
        Command::new(Self::exe())
            .args(["dump", &dev])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------- cgd

/// NetBSD の cgd。
///
/// **他の三つと成り立ちが違う。** LUKS も geli も softraid も、暗号化された
/// ディスクの中に鍵の入れ物を持っている。cgd は持たない。ディスクには何も
/// 書かれず、鍵をどう作るかは params ファイルの側にある。
///
/// 素直に `storedkey`（鍵を params に書く）を使うと、消去は普通のファイルを
/// 消すことになる。**それは crypto-erase を選んだ理由そのものに反する。**
/// SD と SSD ではウェアレベリングのせいで削除が消去として成立しないから
/// 鍵を壊す方式にしたのに、その鍵がファイルとして置かれていては元の木阿弥。
///
/// なので `shell_cmd` を使う。鍵を手元に置かず、開くたびに control plane から
/// 取る。**消す物が手元に無ければ、消えたかどうかを flash の挙動に賭けずに
/// 済む。** 代わりに開くたびに網が要り、消去の保証は control plane に乗る。
///
/// 実機で確かめた（2026-09-22）。block 形式で、鍵は生の 32 バイトを
/// そのまま stdout に出せばよい。
///
/// ```text
/// keygen shell_cmd {
///     cmd "sarachi-recovery key-fetch --device-id …";
/// };
/// ```
pub struct Cgd {
    /// 鍵を取ってくるコマンド。params に書き込まれる。
    key_cmd: String,
    /// params ファイルの置き場。root の外（ESP など）に置く。
    ///
    /// **ここに鍵は入らない。** 入っているのは「どう取ってくるか」だけ
    /// なので、読まれても鍵にはならない。
    params: PathBuf,
}

impl Cgd {
    pub fn new(key_cmd: String, params: PathBuf) -> Self {
        Self { key_cmd, params }
    }

    fn exe() -> &'static str {
        first_existing(&["/sbin/cgdconfig", "/usr/sbin/cgdconfig"], "cgdconfig")
    }

    /// params を組み立てる。鍵は入らない。
    fn params_text(&self) -> String {
        format!(
            "algorithm aes-xts;\n\
             iv-method encblkno1;\n\
             keylength 256;\n\
             verify_method none;\n\
             keygen shell_cmd {{\n\
             \tcmd \"{}\";\n\
             }};\n",
            self.key_cmd
        )
    }

    /// 開いた先の名前（cgd0 など）を path から取る。
    fn unit(mapped: &Path) -> String {
        mapped
            .file_name()
            .map(|n| n.to_string_lossy().trim_end_matches('d').to_string())
            .unwrap_or_default()
    }
}

impl Crypt for Cgd {
    fn name(&self) -> &'static str {
        "cgd (aes-xts 256)"
    }

    /// cgd に「容器を作る」操作は無い。ディスクには何も書かれない。
    ///
    /// ここでやるのは params を置くことだけ。渡された鍵は使わない——鍵は
    /// 開くたびに `key_cmd` が取ってくる。呼ぶ側がそれを預けているはず。
    fn format(&self, _device: &Path, _key: &[u8]) -> Result<()> {
        if let Some(d) = self.params.parent() {
            std::fs::create_dir_all(d)
                .with_context(|| format!("params の置き場を作れない: {}", d.display()))?;
        }
        std::fs::write(&self.params, self.params_text())
            .with_context(|| format!("params を書けない: {}", self.params.display()))
    }

    fn open(&self, device: &Path, _key: &[u8], name: &str) -> Result<PathBuf> {
        // 鍵は渡さない。params の shell_cmd が取ってくる。
        let dev = device.to_string_lossy().to_string();
        let params = self.params.to_string_lossy().to_string();
        run(
            Self::exe(),
            "cgd の展開",
            &["-V", "none", name, &dev, &params],
            None,
        )?;
        Ok(PathBuf::from(format!("/dev/{name}d")))
    }

    fn close(&self, mapped: &Path) -> Result<()> {
        let unit = Self::unit(mapped);
        run(Self::exe(), "cgd の閉鎖", &["-u", &unit], None)
    }

    /// **ここでは消せない。**
    ///
    /// ディスク上に鍵材料が無いので、壊す物が無い。黙って成功を返すと
    /// 「消したつもりで消えていない」になる。消去は取り返しがつかない側
    /// ではなく、**消え残る側**に倒れる誤りなので、はっきり断る。
    fn erase(&self, _device: &Path) -> Result<()> {
        bail!(
            "cgd はディスク上に鍵材料を持たないので、ここでは消せない。\n\
             消去は control plane で鍵を失効させること。鍵を手元に置いて\n\
             いない前提の設計なので、失効させれば二度と開かない"
        )
    }

    /// cgd はヘッダを持たないので、見ただけでは分からない。
    ///
    /// params が在るかで代える。**これは弱い判定**で、params を消された
    /// 機体では容器でないと見える。だから消去の側は has_on_disk_key を見て
    /// 別の道を通る。
    fn is_container(&self, _device: &Path) -> bool {
        self.params.exists()
    }

    fn has_on_disk_key(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linuxは実装がある() {
        let c = match for_kind(RootKind::LinuxLuks, &Setup::default()) {
            Ok(c) => c,
            Err(e) => panic!("Linux で実装が見つからない: {e}"),
        };
        assert_eq!(c.name(), "LUKS2");
    }

    #[test]
    fn 実装していないosは断る() {
        // 黙って別の手を使ってはいけない。消去は取り返しがつかないので、
        // 確かめていない経路は通さない。
        // 残るは OpenBSD の softraid crypto だけ。
        let k = RootKind::OpenBsdData;
        let e = match for_kind(k, &Setup::default()) {
            Ok(_) => panic!("{k:?} は実装していないのに通った"),
            Err(e) => e,
        };
        assert!(
            e.to_string().contains("実装していない"),
            "{k:?} の断り方: {e}"
        );
    }

    #[test]
    fn netbsdはcgdで鍵をディスクに置かない() {
        // cgd はディスク上に鍵材料を持たない。素直に storedkey を使うと
        // 消去が普通のファイルを消すことになり、SD や SSD で成立しない。
        // 鍵は開くたびに control plane から取る形にしてある。
        for k in [RootKind::NetBsdCgd, RootKind::NetBsdFfs] {
            let c = for_kind(k, &Setup::default()).expect("cgd の実装が要る");
            assert_eq!(c.name(), "cgd (aes-xts 256)");
            assert!(
                !c.has_on_disk_key(),
                "{k:?} がディスク上に鍵を持つことになっている"
            );
            // ここで消せてしまうと「消したつもりで消えていない」になる。
            let e = match c.erase(Path::new("/dev/null")) {
                Ok(()) => panic!("{k:?} で消せてしまった"),
                Err(e) => e.to_string(),
            };
            assert!(e.contains("control plane"), "断り方: {e}");
        }
    }

    #[test]
    fn cgdのparamsに鍵は入らない() {
        // params は root の外に置く。読まれても鍵にならないことが要。
        let c = Cgd::new(
            "sarachi-recovery key-fetch --device-id dev-1".into(),
            PathBuf::from("/etc/cgd/x"),
        );
        let t = c.params_text();
        assert!(t.contains("keygen shell_cmd"), "{t}");
        assert!(t.contains("key-fetch --device-id dev-1"), "{t}");
        assert!(
            !t.contains("storedkey"),
            "鍵を params に書いてはいけない: {t}"
        );
        assert!(!t.contains("key "), "鍵らしきものが入っている: {t}");
    }

    #[test]
    fn dragonflyはgeliではなくluks() {
        // 実機で確かめた（2026-09-22）。/sbin/geli は存在せず、base に
        // /sbin/cryptsetup と /sbin/dmsetup が在り、dm_target_crypt.ko が
        // 読み込める。FreeBSD 系だからと geli を当てると起動時に必ず失敗する。
        let c = match for_kind(RootKind::DragonFlyLuks, &Setup::default()) {
            Ok(c) => c,
            Err(e) => panic!("DragonFly で実装が見つからない: {e}"),
        };
        assert_eq!(c.name(), "LUKS2", "DragonFly に geli を当ててはいけない");
    }

    #[test]
    fn freebsdは実装がある() {
        for k in [RootKind::FreeBsdZfs, RootKind::FreeBsdUfs] {
            let c = match for_kind(k, &Setup::default()) {
                Ok(c) => c,
                Err(e) => panic!("{k:?} で実装が見つからない: {e}"),
            };
            assert_eq!(c.name(), "geli (AES-XTS 256)");
        }
    }

    #[test]
    fn geliは開いた先から元のproviderに戻せる() {
        // close は open が返した path を受ける。geli は名前を選ばせず
        // <provider>.eli になるので、detach するには .eli を外す必要がある。
        assert_eq!(
            Geli::provider(Path::new("/dev/ada0p3.eli")),
            PathBuf::from("/dev/ada0p3")
        );
        // 既に外れている物を渡されても壊れない。
        assert_eq!(
            Geli::provider(Path::new("/dev/ada0p3")),
            PathBuf::from("/dev/ada0p3")
        );
    }

    /// 実機での破壊的な試験。
    ///
    ///     SARACHI_TEST_DEVICE=/dev/md0 cargo test -- --ignored
    ///
    /// **渡したデバイスの中身は失われる。** 既定では走らない。
    ///
    /// 命令列が通ることと、消えたことは別なので、ここでは後者を見る。
    /// 鍵を破棄した後に同じ鍵で開けてしまわないこと——それが crypto-erase が
    /// 消去として成立している証拠で、設計全体がそこに乗っている。
    #[test]
    #[ignore]
    fn 実機で鍵を破棄すると開かなくなる() {
        use std::io::Write;

        let dev = match std::env::var("SARACHI_TEST_DEVICE") {
            Ok(d) => d,
            Err(_) => panic!("SARACHI_TEST_DEVICE にデバイスを渡すこと"),
        };
        let dev = Path::new(&dev);

        let kind = if cfg!(target_os = "freebsd") {
            RootKind::FreeBsdUfs
        } else if cfg!(target_os = "linux") {
            RootKind::LinuxLuks
        } else {
            panic!("この OS の暗号層はまだ実装していない");
        };
        let cr = for_kind(kind, &Setup::default()).expect("暗号層が要る");
        println!("{} を {} で試す", dev.display(), cr.name());

        let key = generate_key().expect("鍵");
        cr.format(dev, &key).expect("容器を作れない");
        assert!(cr.is_container(dev), "作った直後に容器だと分からない");

        let mapped = cr.open(dev, &key, "sarachi-test").expect("開けない");
        {
            // geli は 4096 の sector で作るので、その倍数で書く。
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .open(&mapped)
                .expect("開いた先に書けない");
            f.write_all(&[0xa5u8; 4096]).expect("書き込み");
            f.sync_all().expect("sync");
        }
        cr.close(&mapped).expect("閉じられない");

        cr.erase(dev).expect("crypto-erase できない");

        // ここが本番。同じ鍵を持っていても開いてはいけない。
        match cr.open(dev, &key, "sarachi-test") {
            Ok(p) => {
                let _ = cr.close(&p);
                panic!("鍵を破棄したのに開いた。crypto-erase が成立していない");
            }
            Err(e) => println!("期待どおり開かない: {e}"),
        }
    }

    #[test]
    fn 鍵は毎回異なり十分に長い() {
        let a = generate_key().unwrap();
        let b = generate_key().unwrap();
        assert_eq!(a.len(), KEY_BYTES);
        assert_ne!(a, b, "同じ鍵が二度出た");
    }
}
