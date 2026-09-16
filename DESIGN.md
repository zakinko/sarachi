# unix-mdm — 設計メモ

2026-09-16 起案

## 目的

Linux および BSD の端末に対して、遠隔からの wipe と再インストールを行う。
Microsoft の MDM（Intune）からも起動できるが、**MS に依存せず単独で成立する**
ことを主軸に据える。WinPE / Autopilot Reset に相当する仕組みを Unix 系で作る。

## 調査で判明した前提（2026-09-16 時点）

### Intune の Linux 経路には remote wipe が無い

himmelblau-idm/intune-spec v0.05 (2026-02-27) による。

- Windows MDM と違い **OMA-DM/SyncML を使っていない**。REST/JSON over HTTPS。
  エンドポイントは LinuxEnrollmentService / LinuxDeviceCheckinService / IWService。
  PKCS#10 CSR で証明書発行、OAuth2 bearer。`RemoteWipe` CSP は存在しない。
- `deviceState` の OData アクションは Retire, SetRD, CheckCompliance, SetOptIn,
  SetHeartBeat, GetManagementState, RegisterForAppPushNotifications,
  RemoveSignedDeviceIdPolicyAssignment, UpdateAadId。**wipe は無い**。

よって MS 側から wipe を起こす経路は次の二つしかない。どちらもアダプタ扱いとする。

1. `CustomConfig/Script` ポリシー — cspPath `com.microsoft.manage.LinuxMdm/CustomConfig/Script`
   に base64 のシェルスクリプトを載せ、ExecutionContext=root で実行できる。
   実質 root のリモート実行チャネル。
2. `Retire` / `GetManagementState` — 管理者が Retire した事実を check-in で検知して自壊する。
   コンソール操作として自然で、1 より堅い。

### Windows の remote wipe は三段階ある（RemoteWipe CSP)

learn.microsoft.com の RemoteWipe CSP による。重要なのは **doWipe が「破壊」ではなく
「初期化して OOBE で戻ってくる」**こと。意味論はそのまま頂く。

| 段 | Windows | 挙動 |
|---|---|---|
| 1 | `AutomaticRedeployment` (Autopilot Reset) | 消して戻す。**登録と Wi-Fi プロファイルを保つ** |
| 2 | `doWipe` / `doWipeCloud` | 消して OOBE で戻す。中断時はロールバックを試みる＝**電源断で回避可能** |
| 3 | `doWipeProtected` | 終わるまで再試行。失敗・中断時はパーティションを消す。**紛失・盗難用**。起動不能もあり得る |

- `doWipe` は「Reset this PC > Remove everything (Clean Data=No, Delete Files=Yes)」と等価。
- `doWipeProtected` は「doWipe は電源を入れ直すだけで簡単に回避できる」ため用意されている。
- `doWipeCloud` (Win11 22H2+) はクラウドから取得して入れ直す。**GitHub Releases 構想と同じ発想**。

→ `WipeOrder` の wipe レベルはこの三段を採用する。
  「電源を抜けば逃げられる wipe」と「意地でも消す wipe」を分ける判断は真似る価値がある。

### WinRE の作り（WinRE technical reference)

1. **RAM ディスク起動** — 「winre.wim 全体を収められる連続した物理メモリが必要」。
   WinRE は自分を丸ごと RAM に載せてから動く。これが「消す主体も消える」循環の解。
   **Linux の initramfs は同じ機構**。kexec が本質ではなく「RAM に全部載せてから走る」が本質。
2. **専用パーティション、root の直後** — winre.wim は specialize パスで回復パーティションへコピーされる。
   理由も明記されている: root に問題があっても起動できる／**BitLocker で暗号化されていても使える**／
   ユーザが誤って消せない。「root の直後に置け（後で広げられるから）」。
3. **更新は置き換え、パッチではない** — 新イメージが丸ごと既存を置換し、実 OS 側から
   boot-critical とインプットデバイスのドライバを注入、カスタマイズ部分を移送する。
   パーティションに収まらない場合の失敗モード（root を縮める／古い回復領域が孤児になる）も
   文書化されている → **回復パーティションは最初に大きめに取る**。
4. **ネットワークは必要な時だけ、順序が決まっている** — Ethernet → 事前設定済み Wi-Fi(Recovery CSP)
   → **OS 側に保存された Wi-Fi プロファイルを流用** → 手動選択。
   MS 自身の但し書き: 繋がらない場合は「**WinRE に Wi-Fi ドライバが無い可能性が高い**」。
   → 天下の MS でも回復環境のドライバ不足に苦しんでいる。これが後述のベース選定の決め手。

WinRE の自動起動条件も参考になる: 連続 2 回の起動失敗、起動完了 2 分以内の予期せぬ停止 2 回、
同 2 分以内の再起動 2 回、Secure Boot エラー、BitLocker エラー。

### Himmelblau（2026-09-16 に実物を clone して実測）

Rust / v5.0.0 / **GPL-3.0-or-later**。18 クレートの workspace。

実測で分かったこと:

- **`libhimmelblau = "0.8.40"` は crates.io の独立クレート**。Himmelblau 本体から完全に分離
  されており、feature に `broker` / `on_behalf_of` / `intune_portal_vers_selection` /
  `pop_support` などを持つ。cli / pam / policies / common / daemon が依存している。
- **`cfg(target_os)` がツリー全体で 0 件**。Linux を暗黙の前提にしている。
  移植は「BSD の枝を埋める」ではなく「**枝分かれを導入する**」作業になる。
- 行数は `common` が 31,206 行で突出（daemon 4,348 / cli 3,665 / policies 2,418）。
  除外したい pam・nss・broker・selinux 等は**合計 5 千行程度しかなく、切っても減らない**。
- TPM/HSM は `kanidm-hsm-crypto ^0.3.6`（Kanidm 由来）経由。**soft バックエンドがここ**。
  BSD では TPM2 TSS が揃わないので soft を使う。
- `broker` は純 Rust の zbus ではなく **C バインディングの `dbus 0.9.11`**（移植が重い）。
  ただし SSO ブローカ用であり wipe には不要。
- `kqueue` と `devd-rs` が vendored 済み。**依存グラフは既に BSD 向けに解決できている**形跡。
- `os-release` の使用は `src/policies/src/compliance_ext.rs` の 1 箇所のみで浅い。

### 決定: Himmelblau は移植しない。`libhimmelblau` に直接依存する

`common` の 31k 行が本体の質量で、そこを通すと 4 万行規模の移植になる。
**`libhimmelblau` に直接依存すればその 31k 行を丸ごと迂回できる。**
欲しいのは Entra 参加と Intune 登録のプロトコル実装だけなので、それで足りる。

- 我々のエージェント = 自前コード + `libhimmelblau`（Intune アダプタ用・**任意**）
  + `kanidm-hsm-crypto`（soft）+ OS 抽象層
- Himmelblau 本体（daemon / pam / nss）は **Linux 専用の認証統合として放置する**
- 結果、BSD と Alpine の移植性は**小さな自前表面の問題**に縮む

副次的に、「OpenBSD に PAM が無い」「musl に NSS が無い」は当面**問題ですらなくなる**
（認証統合をやらないので）。後述の NSS 4 方式の話は、将来 Entra ログインまで広げる時にのみ効く。

検証すべきビルドも「Himmelblau 全部」ではなく
**「`libhimmelblau` 単体が BSD / arm64 で通るか」**に縮小する。桁違いに安い。

→ **実装言語は Rust で確定**。

**ライセンスは問題にならない**（2026-09-16 実測）: `libhimmelblau` は
**LGPL-3.0-or-later** であって、Himmelblau 本体の GPL-3.0-or-later ではない。
本プロジェクトが GPL に縛られることはない。配布元は
`gitlab.com/samba-team/libhimmelblau` で、**Samba チームのプロジェクト**。
Samba は移植性に厳しい文化なので、BSD 対応の見込みとしても悪くない兆候。

依存は 32 件。うち **`openssl` クレート**が BSD 移植で最大の risk（上記）。

## 対象

### Linux（7）

Himmelblau の対応表は Debian, Fedora, Linux Mint, openSUSE, Oracle Linux, RHEL,
Rocky Linux, SLE, Ubuntu, NixOS。

| | 状況 |
|---|---|
| Debian / Fedora / SUSE / Ubuntu | 対応済み |
| AlmaLinux | 表に無いが RHEL/Rocky と同じリビルド。rpm がほぼそのまま通るはず |
| Raspbian | **要対応**。ただしディストリではなく arch と媒体の問題 |
| Alpine | **要対応**。対応表に無い。しかも musl / OpenRC で他の 6 つと性質が違う |

Raspbian が別物である理由:
- arm64 / armhf。Himmelblau の arch 対応は文書化されておらず**未検証**
- TPM が無い → SoftHSM 経路（上記のとおり存在する）
- **SD / eMMC は上書き消去が効かない**。ウェアレベリングで元ブロックが残り、
  ATA secure erase も NVMe format も無い → **crypto-erase 以外に手が無い**

Alpine が別物である理由:
- **musl であって glibc ではない**。Rust の musl ターゲット自体は問題ないが、
  **musl には NSS のモジュール機構が無い**。OpenBSD と同じ形の問題。
  musl の拡張点は `nscd` プロトコルなので、Himmelblau を NSS モジュールではなく
  **nscd プロトコルを喋る側**として書く必要がある。
- **OpenRC**。systemd でも BSD rc.d でもない third の形。
- PAM は `linux-pam` があるので問題なし。
- ただし**これは認証統合の話で、wipe エージェントには無関係**。
  OpenBSD と同じ割り切り（当面は wipe だけ動かす）がそのまま使える。
- 回復環境のベースにも Alpine を使うので（後述）、**ビルド系を共有できる**のは大きな利得。

### NSS 統合は 4 方式に分かれる

認証まで通す場合、ここが BSD 移植と同格の作業量になる。

| 方式 | 対象 |
|---|---|
| glibc NSS モジュール | Debian, Ubuntu, Fedora, Alma, SUSE, Raspbian |
| **nscd プロトコル** | Alpine (musl) |
| nsdispatch | FreeBSD, GhostBSD, NetBSD, DragonFly |
| **機構なし**（BSD Authentication） | OpenBSD |

### BSD（5）

| | PAM | NSS | 備考 |
|---|---|---|---|
| FreeBSD | OpenPAM | nsdispatch（Linux と別形式） | |
| GhostBSD | 同上 | 同上 | FreeBSD 派生。実質パッケージングのみ |
| NetBSD | OpenPAM | 独自 nsswitch | pkgsrc で配布 |
| DragonFly | OpenPAM | FreeBSD 系 | disk 周りが分岐 |
| **OpenBSD** | **無し**（BSD Authentication） | **機構が無い** | 根本的に別物 |

OpenBSD が最大の壁。pam/nss クレートが使えず、login_- スタイルの
BSD Authentication ヘルパとして書き直しになる。
→ **当面は認証統合を諦め、wipe エージェントだけ動かす**割り切りとする。

Himmelblau 側の共通除去項目: selinux / apparmor クレートは無効化、
systemd unit は rc.d/rc.subr に置換、os-release は BSD に無いので OS 判定を差し替え、
zbus(D-Bus) は ports/pkgsrc にあるが依存を切れるなら切る。

## アーキテクチャ

```
┌─ コマンド源 (pluggable) ──────────────────────────┐
│  A. 自前 control plane (mTLS + 署名付きコマンド)  ← 既定・MS非依存
│  B. Intune アダプタ (CustomConfig/Script, Retire 検知) ← 任意
│  C. ローカル/手動 (署名済み wipe チケット)         ← 保険経路
└──────────────┬────────────────────────────────┘
               │ 署名検証された WipeOrder
        ┌──────▼──────┐
        │  wipe-agent   │  常駐。11 ターゲット共通。OS 抽象層を持つ
        └──────┬──────┘
               │ 回復パーティションへ pivot
        ┌──────▼───────────┐
        │  recovery env      │  WinPE 相当。消去 → 取得 → 書き戻し
        └────────────────────┘
```

### 設計方針

1. **コマンドは署名付き `WipeOrder` に正規化**する。nonce・有効期限・対象デバイス ID・
   wipe レベル・発行者署名を持つ。エージェント中核は MS を一切知らない。
2. **多重トリガ**。ネットワーク遮断で止められては意味がないので、常駐ポーリング、
   一定期間 check-in が無ければ自壊する dead-man timer、事前署名チケットの手動投入。
3. **全 OS で FDE 前提の crypto-erase に統一**。Pi を含める以上これは推奨ではなく必須。
   物理消去は「暗号化していない実機向けの追加オプション」に格下げする。

### 消去手段

| | crypto-erase | 物理消去 | pivot |
|---|---|---|---|
| Linux | `cryptsetup luksErase` | `blkdiscard` / `nvme format` / `hdparm` | kexec + initramfs |
| FreeBSD / GhostBSD | geli 鍵破棄 | `camcontrol security` / `nvmecontrol format` | 最小 root へ reboot |
| NetBSD | cgd 鍵破棄 | `dkctl discard` / `atactl` | 最小 root へ reboot |
| DragonFly | dm_target_crypt (LUKS) ※**要検証** | FreeBSD 系ツール | 最小 root へ reboot |
| OpenBSD | softraid crypto 鍵破棄 (`bioctl`) | `dd` 主体 | **kexec 相当が無い** |
| Raspberry Pi | 同上（唯一の手段） | **不可**（ウェアレベリング） | — |

## 再インストール

### 回復環境の置き場所

ディスクを消すと再インストールする主体も消える、という循環がある。
**WinRE の答えは「RAM ディスク起動」** で、自分を丸ごと RAM に載せてから走る。
Linux の initramfs がそのまま同じ機構なので、これを採る。

置き場所は **回復パーティション（root の直後、大きめに確保）**。WinRE と同じ理由:
root が壊れていても起動できる、FDE で暗号化されていても使える、ユーザが誤って消せない。
持ち出したノートを遠隔で消して再導入する場面で唯一動くのもこれ（PXE は社内 LAN 限定）。

起動時に initramfs を全て RAM に展開してしまえば、**回復パーティション自身も含めて
ディスク全体を解放できる**。BSD に kexec が無い件は、これで問題でなくなる
（kexec は本質ではなかった）。

チェーン方法は UEFI + EFI stub カーネルを主軸とする。FreeBSD には efibootmgr(8) があり、
NetBSD/OpenBSD も EFI 変数を書けばよい。Raspberry Pi は FAT の起動パーティションを
差し替えるだけで済む。BIOS/MBR 機はチェインローダが要る（**要調査**）。

### ベース: Alpine Linux の initramfs、全ターゲット共通で 1 つ

**回復環境は、書き戻す先の OS と一致している必要がない。** 署名検証・消去・取得・dd・再起動
しかしないので、FreeBSD のイメージを Linux から dd して何の問題もない。
これに気づくと OS ファミリごとに 4〜5 種類作る理由が消える。

Linux を選ぶ決め手は**ドライバとファームウェアの網羅性**。ホテルの Wi-Fi から回復させたいのに
無線チップが動かなければ全てが無意味になる。`linux-firmware` + `wpa_supplicant`/`iwd` より
広い網は無く、任意の最近のノートの無線を NetBSD/OpenBSD で動かすのは賭けになる。
上記の「MS ですら WinRE のドライバ不足に苦しんでいる」が傍証。
`cryptsetup` / `nvme-cli` / `hdparm` / `blkdiscard` が揃うのも Linux。

ディストリは **Alpine**。当初 Buildroot と決めたが、**Alpine がターゲット OS に加わったので
前提が変わった**（2026-09-16 に変更）。

- 同じビルド系が**回復 initramfs と Alpine の minimum-image の両方**を吐く
- musl 向け Rust 静的ビルドの作業を共有できる
- `apk` で cryptsetup / nvme-cli / wpa_supplicant / linux-firmware が即座に揃う
- Alpine は x86_64 / aarch64 / armv7 / armhf を公式にビルドしており、
  **Raspbian の arch 要件も同時に満たす**

Buildroot の利点だった再現性は、apk のバージョンとリポジトリスナップショットを固定すれば
実用上足りる。**工具を一系統に畳める利得のほうが大きい**と判断した。

### 動作手順（RAM 常駐）

1. UEFI が回復パーティションからカーネル + initramfs を読む
2. カーネルが initramfs を **RAM 上の tmpfs に全展開する。root は一切マウントしない**（pivot もしない）
3. この時点でディスクは完全に手が空く — **回復パーティション自身を含めて**消せる
4. ネットワークを上げる → 署名検証 → 消去 → ダウンロード → dd → 再起動

**イメージを RAM に溜めないこと**が肝。`curl | zstd -dc | dd` で流し込むので、
必要な RAM は initramfs のサイズだけで、書き戻すイメージの大きさとは無関係になる。
数百 MB のイメージを 2GB の Pi に書ける。

RAM の食い手はむしろ**ファームウェア**。`linux-firmware` は丸ごとだと GB 級なので、
無線・NIC・ストレージに絞り込む必要がある（**実測要**）。

役割分担: crypto-erase は**稼働中の OS 側でネイティブのツール**（`geli` / `cgd` / `bioctl` /
`cryptsetup`）で行うのが主経路。この回復環境は粗い消去と書き戻しを担当する。

ネットワーク接続の順序は WinRE に倣う: Ethernet → 事前設定済み Wi-Fi → 実 OS から
Wi-Fi プロファイルを流用 → 手動選択。**Wi-Fi プロファイルの引き継ぎは持ち出し端末で必須**。

回復環境の更新も WinRE に倣い、**in-place パッチではなく丸ごと置き換え**とする。

### 成果物は 2 種類

1. **recovery env** — 回復パーティション常駐の最小環境。消去と書き戻しのみ。50〜200MB。
2. **minimum-image** — 書き戻されるクリーンな OS。OS×arch ごと。数百 MB。

**上流の base セットから組む**（FreeBSD base.txz / NetBSD sets / debootstrap minbase /
Alpine minirootfs）。ゼロから作らず、上流のクラウドイメージ丸ごとでもない、中間を取る。

### 配布は GitHub Releases

制約（確認済み）: **2 GiB/ファイル、1000 アセット/リリース、総容量・帯域とも上限なし**。

- minimum-image は数百 MB なので 2 GiB に一桁以上の余裕がある。制約は実質効かない。
- 1000 アセット入るので **Linux 6 × arch + BSD 5 の全マトリクスが 1 リリースに収まる**。
- CDN 配信の公開 HTTPS なので、**社外のネットワークからでも取れる**。
  これが PXE の「持ち出し端末に無力」問題を解く。イメージサーバも VPN も要らない。
- Range リクエストが効くので、回復環境は**中断したら再開する**実装にすること。
  数百 MB を悪い回線で落とすので、やり直しは致命的。

### 同一性の再生成（Windows で言う sysprep /generalize）

**dd はパーティションもファイルシステムも、それ以上のものも複製する。**
焼いたままでは全台が同じ同一性を持つので、初回起動前に振り直す工程が要る。

| 層 | dd で複製されるもの | 再生成 |
|---|---|---|
| GPT | ディスク GUID、各パーティション GUID | `sgdisk -G` / `gpart` |
| FS | ext4/xfs/btrfs UUID、FAT ボリュームシリアル、ZFS pool GUID、UFS fsid | `tune2fs -U random` / `xfs_admin -U` / `btrfstune -U` |
| **暗号** | **LUKS マスター鍵**と UUID、geli 鍵、cgd 鍵 | **後述。後から変更できない** |
| ホスト同一性 | `machine-id`、D-Bus machine-id、**SSH ホスト鍵**、random-seed | 削除して初回起動で再生成 |
| 登録 | Entra/Intune のデバイス ID と証明書 | **イメージに入れない**（Intune もクローンを非対応と明記） |
| ネットワーク | ホスト名、DHCP DUID、永続 NIC 名 | 再生成 |

SSH ホスト鍵が全台同一というだけでも十分まずい。

### 暗号化済みイメージを dd で配ることは原理的にできない

消去モデルは「鍵を破棄すれば読めなくなる」に全面的に依存している。ところが
**dd で焼いた全台が同じ LUKS マスター鍵を持つ**と、一台で鍵を破棄しても
**同じイメージを持つ者は誰でもそのディスクを復号できる**。消去したことにならない。

そして **LUKS のマスター鍵は後から変更できない**。パスフレーズは変えられるが、
マスター鍵は `cryptsetup-reencrypt` で全体を暗号化し直すしかなく、遅くて危険。

→ **イメージを「全ディスク像」から「rootfs 像」に変える。**

- 回復環境がパーティションを切る → **その場で新しい鍵で LUKS/geli/cgd を作る**
  → 中に rootfs を展開 → ブートローダを入れる
- 鍵は回復環境が生成するので、**構造上、台ごとに必ず異なる**。イメージに鍵は入らない
- 副産物: UUID 衝突が起きない（回復環境が新規に振る）、**後から広げる処理が不要**
  （最初から実ディスクのサイズで切る）、イメージも小さくなる

代償は**ブートローダ導入を OS ごとに実装する**こと。dd 一本で全 OS を貫く利点の一部を手放す。
ただし選択の余地は少ない: dd の単純さを守ると crypto-erase が成立せず、
その crypto-erase は Pi（SD なので物理消去不可）では**唯一の消去手段**であるため。

### パーティション配置

```
1. ESP       FAT32  512MB   EFI System Partition。実 OS と回復環境の双方のローダを置く
2. recovery  ext4   2GB     カーネル + initramfs（WinPE 相当）。署名付き read-only
3. root      残り            LUKS コンテナ。回復環境が台ごとに新しい鍵で作る
```

**WinRE と違い recovery を root の前に置く。** WinRE が Windows パーティションの直後に
置くのは、Windows を縮めて回復領域を広げられるようにするため。こちらは回復環境が
固定サイズの成果物で丸ごと置換され、root は回復環境が実ディスクに合わせて切るので、
前に置いたほうが root を末尾まで自由に伸ばせる。

サイズは 2GB と大きめに取る。WinRE が「新イメージが既存パーティションに収まらない」
失敗モードを文書化しているのが教訓（root を縮める／古い回復領域が孤児になる）。

## セキュリティ上の急所

**ここが設計全体で一番危険な場所。** 「遠隔 wipe + 再インストール」は、
裏返せば**遠隔でルートキットを配る経路**そのものである。

1. **イメージに秘密を焼かない。** 公開リリースなので、登録情報を埋めれば全世界に配ることになる。
   登録は初回起動時にトークンで行う。
2. **署名鍵を CI に置かない。** GitHub のシークレットに署名鍵を置くと、
   GitHub を一つ落とせばイメージと署名の両方が手に入り、署名の意味が消える。
   鍵はオフライン保持。**公開鍵は recovery パーティションに焼き込んで**検証する。
   信頼の根を GitHub の外に置く。
3. 署名方式は Ed25519 の signify / minisign 系が候補。実装が小さく、
   OpenBSD と NetBSD には `signify(1)` が native にあり、Rust の crate もある。
   **signify と minisign の署名フォーマット互換性は要確認。**

## 未検証事項

- **`libhimmelblau` 単体が BSD / arm64 でビルドできるか**（最重要。これが通れば道が開ける）。
  依存に **`openssl` クレート**（rustls ではない）が入っているのが最大の risk。
  FreeBSD は base に OpenSSL があるが、**NetBSD と OpenBSD は LibreSSL** で、
  `openssl` クレートの LibreSSL 対応はバージョンに敏感。Alpine/musl も要確認。
- 回復パーティションの GPT タイプ GUID（XBOOTLDR を使うか独自を振るか）。
- ブートローダ導入を OS ごとにどう実装するか（rootfs 像方式の代償）。
- `kanidm-hsm-crypto` の soft バックエンドが BSD で通るか。
- NetBSD を Mac arm の brew qemu で動かすための当て物の内容。
- DragonFly の LUKS (dm_target_crypt) 対応。
- signify / minisign の署名フォーマット互換性。
- 各 base セットの実サイズ（本文中の数値は目安）。
- **Secure Boot**。回復環境のカーネルに署名が要る。shim を MS UEFI CA に署名してもらうか、
  MOK で自前鍵を登録するか、Secure Boot を切るか。本番運用では避けて通れない。
- BIOS/MBR 機で回復パーティションへチェーンする方法。
- 回復環境が必要とする RAM 量（RAM 常駐なので下限が決まる）。低スペック機と Pi で要確認。
- `linux-firmware` の絞り込み範囲と、絞った結果の initramfs 実サイズ。
- musl の nscd プロトコルで Himmelblau の NSS 相当をどこまで賄えるか。

## 開発環境（2026-09-16）

ホスト: **Apple M4 / macOS 26.6.2 / arm64**。lima・qemu(aarch64/x86_64)・VMware Fusion あり。
Rust は未導入。

**arm64 先行で進める。** 12 標的のうち 9 つが M4 上でネイティブ速度で動き、
かつ arm64 は「未検証で一番危ない標的」そのものなので一石二鳥になる。

| M4 でネイティブ | 落ちるもの |
|---|---|
| Alpine / Debian / Ubuntu / Fedora / SUSE / Alma（全て arm64 あり）<br>Raspbian 64bit、FreeBSD arm64 (Tier 1)、NetBSD evbarm aarch64、OpenBSD arm64 | **DragonFly** — x86_64 のみ。AArch64 移植は存在せず bounty 段階<br>**GhostBSD** — 公式は amd64。Pi 向け arm64 は有志ビルドのみ<br>**armhf (32bit Raspberry Pi OS)** — Apple Silicon は AArch32 を実装しないので arm64 ホストでも動かない |

落ちる 3 つはエミュレーション（qemu TCG、実用に耐えない遅さ）か実機が要る。
ただし 3 つとも**後回しにできる**: GhostBSD は FreeBSD 派生で FreeBSD/arm64 の成果がほぼ乗り、
DragonFly と armhf は x86 機（実機 / 別の実機）か実 Pi にまとめて当てればよい。
→ **arm64 先行で 9 割方が進み、残り 3 つを最後に一括**。

### 既知の罠

- **NetBSD を Mac arm の brew 版 qemu で動かすには当て物が要る**（ユーザ実体験）。
  内容は**要記録** — 次に NetBSD VM を立てる時に再発するので、詳細をここに書き残すこと。

## 当面の進め方

1. **`libhimmelblau` 単体**を Alpine aarch64 と FreeBSD arm64 でビルドし、
   どこで壊れるかを実測する（Himmelblau 全体ではない。上記の決定を参照）。
2. 並行して、MS 非依存の `WipeOrder`（署名付きコマンド）とエージェント骨格を Rust で起こす。
3. crypto-erase を Linux / FreeBSD の 2 つで先に通し、OS 抽象層の形を確定させてから
   残り 4 BSD に広げる。
