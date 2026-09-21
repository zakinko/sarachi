# sarachi

Linux と BSD の端末を遠隔から消去し、入れ直すための仕組み。

名前は更地から。この仕組みがやるのは消すことだけではなく、**消して、均して、
次が建つ状態にして返す**ことで、更地はその状態そのものを指す。Windows の
`doWipe` が「破壊」ではなく「初期化して OOBE で戻ってくる」だったという、
設計の出発点とも重なる。Microsoft の
MDM（Intune）からも起動できるが、**MS に依存せず単独で成立する**ことを主軸に
据えている。Windows の WinRE / Autopilot Reset に相当するものを Unix 系で作る。

設計の全体と、そこへ至った判断の理由は [DESIGN.md](DESIGN.md) にある。
以下はその要約。

## なぜ MS 非依存が主軸なのか

Intune の Linux 経路には remote wipe が**無い**。Windows MDM と違って
OMA-DM/SyncML ではなく REST/JSON で、`deviceState` が返す OData アクションは
`Retire` や `CheckCompliance` などで、`RemoteWipe` CSP に相当するものが存在しない。
公式の Intune エージェントも Ubuntu 24.04/26.04 と RHEL 9/10 の x86_64 かつ
GNOME 必須で、12 ある標的のうち 2 つしか覆わない。

そのため自前の control plane を本線に据え、Intune は `CustomConfig/Script` と
`Retire` 検知という二つのアダプタとしてぶら下げる。

## 消去の三段階

Windows の `RemoteWipe` CSP をそのまま意味論として採っている。Windows が
`doWipe` と `doWipeProtected` を分けているのは、前者が電源を入れ直すだけで
回避できるためで、紛失・盗難時には後者を使えと仕様に明記されている。

| 段 | Windows での対応物 | 挙動 |
|---|---|---|
| Reset | `AutomaticRedeployment` | 消して戻す。登録と Wi-Fi は保つ |
| Factory | `doWipe` / `doWipeCloud` | 初期状態まで戻す。電源断で回避できる |
| Destroy | `doWipeProtected` | 終わるまで再試行。回復領域も消す |

## 構成

```
crates/disk       GPT の配置と書き出し。CRC-32 も自前
crates/order      署名付き消去命令。上の三段に対応
crates/image      署名付きマニフェストと、書き込み前に検証する取り込み
crates/recovery   回復環境で走る実行体
recovery-env/     WinPE 相当の initramfs を組む
patches/          上流へ出す候補の当て物（まだ送っていない）
```

## 今どこまで動くか

**一周が閉じている。** 外部媒体から起動した回復環境が、ESP と回復領域と root を
埋め、そのディスクだけで起動し直すと回復環境が自分で立ち上がる。

```
[外部媒体]  署名検証 → GPT → ESP → 回復領域 → LUKS2(台ごとの鍵) → rootfs
[自己起動]  UEFI → systemd-boot → カーネル+initramfs → 回復環境
[消去]      Factory: crypto-erase / Destroy: + ヘッダ + 回復領域 + GPT
```

外から確かめたこと: 導入後に回復環境が作った鍵で LUKS を開いて中身が読め、
別の鍵では開かない。Factory 消去の後は同じ鍵でも
`No usable keyslot is available.` になる。Destroy の後は主・予備とも GPT が消える。
外部媒体を繋がずディスク一つで起動して回復環境が立ち上がる。

確かめていないこと、未着手のものは [DESIGN.md](DESIGN.md) の「未検証事項」に
まとめてある。実機のカーネルとファームウェア、FreeBSD 以外の BSD、
Secure Boot、鍵をどこに預けるか、あたりが残っている。

## 回復環境

WinRE と同じく、自分を丸ごと RAM に載せてから走る。消去の対象に自分が
乗っていると、消した瞬間に自分が消えるため。Linux の initramfs がそのまま
同じ機構になる。起動後に `root fs` が `rootfs` と出るのがその証拠にあたる。

回復環境は**書き戻す先の OS と一致している必要がない**。やるのは署名検証と
消去と取得と書き込みだけで、対象 OS の中身を知らないため。全標的で一つの
Alpine ベースの実行体を使う。Linux を選んだのはドライバとファームウェアの
網羅性で、無線が動かなければ遠隔からの回復は成立しない。

```
UEFI → systemd-boot → カーネル → initramfs(RAM) → /init
  → モジュール読み込み → ネットワーク → 署名検証 → 消去 → 取得 → 書き込み
```

## 作る・試す

回復環境のビルドは Alpine の上で走らせる。

```sh
recovery-env/alpine/build.sh     # initramfs を組む（Alpine の上で）
recovery-env/alpine/bundle.sh    # 配る三つの像を作る
cargo test --workspace           # 全クレートの試験
cargo run -p sarachi-disk --example plan                # 配置を見る（何も書かない）
cargo run -p sarachi-image --example sign -- keygen .   # 署名鍵を作る
cargo run -p sarachi-image --example sign -- sign rootfs.img signing.key
```

回復環境の側:

```
sarachi-recovery list
sarachi-recovery plan <dev>
sarachi-recovery wipe <dev> --level reset|factory|destroy [--commit]
sarachi-recovery provision <dev> --base <url> --key <pub> \
                  --esp esp --recovery recovery --rootfs rootfs [--commit]
```

`sarachi-recovery` は既定で何も書かない。`--commit` を明示しない限り、
何をするつもりかを表示して終わる。

## 対象

Linux 7（Alma, Debian, SUSE, Raspbian, Fedora, Ubuntu, Alpine）と
BSD 5（FreeBSD, NetBSD, OpenBSD, DragonFly, GhostBSD）。
どこまで実際に確かめたかは [DESIGN.md](DESIGN.md) の「到達点」を見ること。
