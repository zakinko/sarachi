# 上流で直すべきもの

この repo で回避しているが、**本当は向こうを直すのが筋**のもの。回避を
入れた時点でここに書く。書かないと、回避が既定になって理由が忘れられる。

「送った」「返事待ち」まで含めて状態を持つ。まだ何も送っていない。送るときは
`~/.claude/CLAUDE.md` の決まりに従って、下書きを見てもらってからにする。

---

## DragonFly

### lang/rust が 1.85.1 で止まっている

**状態**: こちらで 1.86 / 1.87 / 1.88 を建てて公開済み。まだ持ちかけていない。

DPorts の `lang/rust` は 2025-04-05 から動いていない。その前は 2024-09 の
1.79 で、一年空いている。`Makefile.DragonFly` が bootstrap の取得元に指して
いる `leaf.dragonflybsd.org/~mneumann/rust/` も 1.84.1 が最後。

止まる理由は構造的なもの。rustc は一つ前の版の rustc でしか建たず、DragonFly
は tier 3 で本家が binary を出さないので、誰かが手で chain を繋ぐしかない。
繋ぐ人が止まれば止まる。

建てた物は mneumann 氏の形に名前を揃えてある。`Makefile.DragonFly` がその
置き場を `MASTER_SITES` に持っているので、**名前を変えずにそのまま使える**。

	https://github.com/zakinko/netbsd-ci-images/releases/tag/dragonfly-rust

持ちかけるなら、tarball そのものより **CI で建て直せる形**（この repo の
`.github/workflows/dragonfly-rust.yml` と `ci/dragonfly-rust-build.sh`）の方が
価値がある。一段 40 分で、六時間の上限に収まる。

### pkg install libssh2 が rust と git と curl を消す

**状態**: 未報告。

`libssh2` 1.11.1 が repo に入っているが、それに合わせて建て直した `rust`
`git` `curl` が無い。`pkg install libssh2` すると upgrade が起き、古い ABI に
依存する三つが巻き添えで消える。

	Installed packages to be REMOVED:
	    curl: 8.10.0
	    git: 2.49.0
	    rust: 1.85.1

2026-09-22 の `dragonfly:6.4:x86:64` repo で確認。repo が移行中の状態で、
利用者から見ると「一つ入れたら三つ消えた」になる。

### sha256 に -c が無い

**状態**: 未報告。小さい。

FreeBSD の `sha256` は `-c digest` で照合できるが、DragonFly の `sbin/md5` は
`getopt` が `"hb:e:pqrs:tx"` で `c` を受けない。FreeBSD 由来の script を持って
くると、ここで黙って落ちる。

回避は `-q` で値を出して自分で比べること。`ci/dragonfly-rust-build.sh` がそう
している。

### -sys crate の C に -fPIC が付かない

**状態**: 未解決。**単独では再現しない**ところまで詰めた。報告先はまだ決められない。

DragonFly は既定で PIE を作るが、rust の build が `cargo` を建てるときに
`libssh2-sys` の object がこうなる。

	liblibssh2_sys-….rlib(agent.o): relocation R_X86_64_32 against
	  .rodata.str1.1 can not be used when making a PIE object

#### 潰した仮説（どれも外れ）

| 仮説 | 確かめ方 | 結果 |
|---|---|---|
| cc-rs が dragonfly に `-fPIC` を付けない | `cc` 1.1.22 の source を読む | 付ける。除外は windows / `-none-` / uefi だけ |
| rust が `CRATE_CC_NO_DEFAULTS` を立てている | bootstrap の source を読む | 立てていない |
| `libssh2-sys` が `.pic(false)` する | `build.rs` 全体を読む | `pic` に一切触れていない |
| cc の版が違う（1.4.7 と 1.1.22） | 実機で 1.1.22 に固定して単独ビルド | 再現せず |
| bootstrap が立てる空の `CFLAGS_<triple>` が既定を止める | 同じ環境変数を立てて単独ビルド | `-fPIC` は渡る |

#### 分かっていること（実機で測った）

**単独の `cargo` では、cc-rs は正しく `-fPIC` を渡し、正しい object が出る。**

	running: "cc" "-O0" "-ffunction-sections" "-fdata-sections" "-fPIC"
	  "-gdwarf-2" … -o …/agent.o -c libssh2/src/agent.c

出来た object を節ごとに数えた結果（`cc` 1.1.22、DragonFly 6.4.2）。

	load される節   非 PIC 0    PIC 向け 112
	.debug_info 等  非 PIC 684

**`.debug_info` の 684 件は `-g` を付ければ正常に入るもの**で、PIE の妨げに
ならない。ここを混ぜて数えると、正常な object が壊れて見える。一度そう読んで
誤った結論を出しかけた。

#### 残っている問い

**rust の bootstrap が `cargo` を建てるときだけ壊れる。何が違うのか。**

本番で `CC_ENABLE_DEBUG_OUTPUT=1` を付けて走らせ、`agent.c` の command line を
捕まえるところまで作った（`ci/dragonfly-rust-build.sh` の `SARACHI_PIC_PROBE`）。
ただし **run が失敗すると vmactions が作業結果を持ち帰らない**ので、まだ回収
できていない。次に触るときは、下調べの側を `exit 0` で終わらせること。

#### 回避

`LIBSSH2_SYS_USE_PKG_CONFIG=1` で C を建てさせない。**回避であって解決では
ない。** 代償として、出来た `cargo` は package の `libssh2` を要る。

---

## NetBSD

### cgdconfig(8) が shell_cmd の鍵の形を書いていない

**状態**: 未報告。文書の話。

`keygen shell_cmd` は「コマンドの stdout から鍵を読む」としか書いていない。
何を出せばよいのか——生のバイトか、base64 か、長さは——が分からない。

実機で確かめた結果は **`keylength` を 8 で割った長さの生のバイト**（256 bit
なら 32 バイト）。書き方も `keygen shell_cmd "cmd";` ではなく block 形式で、
`cmd` は PARAMETERS FILE の側に載っている。

	keygen shell_cmd {
		cmd "…";
	};

man に一行足せば、次の人が実機で試さずに済む。

---

## crates.io

### yoke-derive 0.8.3 が古い rustc で建たなくなった

**状態**: 未報告。上流の退行。

0.8.2 は `core::str::from_utf8` と正しく書いていたのが、0.8.3 で裸の
`str::from_utf8` になった。後者は 1.87 で `str` の固有関連関数になって初めて
通る書き方で、**それ以前の rustc では建たない**。

	error[E0599]: no function or associated item named `from_utf8`
	  found for type `str` in the current scope

`rust-version` は上がっていないので、MSRV 解決でも避けられない。module path で
書けばどの版でも通るので、直すのは一語。icu4x に出すのが筋。

### libkrimes の getdomainname が BSD で通らない

**状態**: 当て物は `patches/` にある。送るのは保留中（利用者の判断）。

詳しくは `patches/README.md`。
