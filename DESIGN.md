# sarachi — 設計メモ

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

ただし前述のとおり、`libhimmelblau` に直接依存する方針を採ったので、
**wipe エージェントだけを動かす限りこの表は効かない**。認証統合まで
広げる時に初めて問題になる。

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

### 消去モデルの実証（2026-09-17）

「鍵を破棄すれば読めなくなる」が設計全体の土台なので、実際に確かめた。

1. LUKS2 を作り、平文の目印を書き込む
2. 生デバイスに平文が見えないことを確認（暗号化が効いている）
3. 鍵があれば読めることを確認
4. `cryptsetup luksErase` で鍵スロットを破棄
5. **同じ鍵でもう開けない**ことを確認 → master key は復元不能

所要時間（Alpine aarch64 / M4 の NVMe 上の loop デバイス）:

| | |
|---|---|
| crypto-erase（4GiB ボリューム） | **20 ms** |
| 全面上書き（4GiB） | 9,260 ms |
| 比 | **463 倍** |

crypto-erase は**容量に依存しない**（ヘッダのみ書き換える）ので、512GiB でも
20ms のまま。上書きのほうは単純比例で約 19 分になる。
**ただしこれは NVMe 上の loop デバイスでの値で、実機の SD や HDD ではもっと開く。**
そして SD と SSD では、そもそも上書きしてもウェアレベリングで元ブロックが残るため、
時間の問題以前に消去として成立しない。

計測時の罠: busybox の `date` は `%N`（ナノ秒）に対応しておらず、黙って秒だけを
返すので測定値が 0 になる。`/proc/uptime` を使うこと。また `count` を指定しない
`dd` は ENOSPC で非ゼロ終了するので、`set -e` のスクリプトはそこで落ちる。

## セキュリティ上の急所

**ここが設計全体で一番危険な場所。** 「遠隔 wipe + 再インストール」は、
裏返せば**遠隔でルートキットを配る経路**そのものである。

1. **イメージに秘密を焼かない。** 公開リリースなので、登録情報を埋めれば全世界に配ることになる。
   登録は初回起動時にトークンで行う。
2. **署名鍵を CI に置かない。** GitHub のシークレットに署名鍵を置くと、
   GitHub を一つ落とせばイメージと署名の両方が手に入り、署名の意味が消える。
   鍵はオフライン保持。**公開鍵は recovery パーティションに焼き込んで**検証する。
   信頼の根を GitHub の外に置く。
3. 署名方式は **Ed25519 を純 Rust（`ed25519-dalek`）で**実装した（2026-09-17）。
   OpenSSL に依存しないので回復環境を小さく保て、NetBSD と OpenBSD の
   LibreSSL 問題もここでは踏まない。signify / minisign との相互運用は
   将来必要になったら考える（現状は自前形式）。

4. **大きなイメージの署名は末尾でまとめて検証してはいけない。** 検証が終わる
   頃には未検証のデータを既にディスクへ書き終えている。チャンクごとの
   SHA-256 を署名済みマニフェストに入れ、**各チャンクを書く前に照合する**。
   署名の検証は小さなマニフェストに一度だけで済み、未検証のバイトが
   ディスクに触れることはない。照合に失敗したらその場で止めるので、
   書かれているのは検証を通った前半だけになり、そこから再開できる。
   （`crates/image` で実装・試験済み）

## 未検証事項

- ~~`libhimmelblau` 単体が BSD / arm64 でビルドできるか~~ → **Alpine aarch64 musl と
  FreeBSD arm64 で実証済み**（FreeBSD は `patches/` の当て物が要る）。
- **OpenBSD での `openssl` クレート**。LibreSSL 4.3.0 は `openssl-sys 0.9.117` の
  対応上端なので通るはずだが、実際に建てていない。**cross では確かめられない**
  （上記のとおり `openssl-sys` が標的側の OpenSSL を要る）ので実機が要る。
  → `vmactions/openbsd-vm` の 7.9 amd64（KVM で走る）。
- NetBSD での **`aws-lc-sys`**。cross では標的の C toolchain が無くて落ちた。
  実機なら通るのか、それとも移植性の問題があるのかは未確認。
- DragonFly と GhostBSD でのビルド。
  → `vmactions/dragonflybsd-vm` 6.4.2、`vmactions/ghostbsd-vm` 26.1。

なお `libkrimes` 単体は **DragonFly と OpenBSD を含む四つの BSD 向けに
手元で建つところまで確認済み**（`patches/README.md` の表）。上の未検証は
`libhimmelblau` 本体の話。
- 回復パーティションの GPT タイプ GUID（XBOOTLDR を使うか独自を振るか）。
- ブートローダ導入を OS ごとにどう実装するか（rootfs 像方式の代償）。
- `kanidm-hsm-crypto` の soft バックエンドが BSD で通るか。
- NetBSD を Mac arm の brew qemu で動かすための当て物の内容。
- DragonFly の LUKS (dm_target_crypt) 対応。
- signify / minisign の署名フォーマット互換性。
- 各 base セットの実サイズ（本文中の数値は目安）。
- **Secure Boot**。UKI にすれば署名対象は単一 PE に畳めるが、その PE を誰の鍵で
  署名するかは残る（shim を MS UEFI CA に署名してもらうか、MOK で自前鍵を登録するか）。
- Alpine で `linuxaa64.efi.stub` を得る方法（apk に見当たらない）。
- 実機用の `lts` カーネルと `linux-firmware` を絞った後の実サイズ。
- BIOS/MBR 機で回復パーティションへチェーンする方法。
- 回復環境が必要とする RAM 量（RAM 常駐なので下限が決まる）。低スペック機と Pi で要確認。
- `linux-firmware` の絞り込み範囲と、絞った結果の initramfs 実サイズ。
- musl の nscd プロトコルで Himmelblau の NSS 相当をどこまで賄えるか。

## 到達点（2026-09-17 時点）

**一周が閉じた。** 導入した機体が、外部媒体なしで自分の回復環境を起動できる。

### 一周の全体

```
[外部媒体から起動した回復環境]
  → 三つの像の署名を全て検証（ディスクに触る前に）
  → GPT を書く
  → ESP に ローダ+カーネル+initramfs を書く
  → 回復領域に ファームウェア を書く
  → root に 台ごとの新しい鍵で LUKS2 を作る
  → 検証しながら rootfs を中へ書く

[そのディスクだけで起動]
  → UEFI → systemd-boot（書いた ESP から）→ カーネル+initramfs
  → 回復環境が RAM 上に立ち上がる（root fs: rootfs）
  → ネットワークも上がる
  → **以後この機体は自分で回復できる**

[消去]
  → Factory: crypto-erase。GPT と回復領域は残る
  → Destroy: crypto-erase + LUKS ヘッダ + 回復領域 + GPT（主・予備とも）
```

### 外から確かめたこと

- 導入後、**回復環境が生成した鍵で LUKS を開き**、中の ext4 をマウントして
  目印のファイルが読めた。**別の鍵では開かない**。
- Factory 段階の後、その鍵で開こうとすると
  `No usable keyslot is available.` — **復元不能**。
- Destroy 段階の後、主 GPT も予備 GPT も消えている。
- **外部媒体を一切繋がず `full.img` だけで起動**し、回復環境が立ち上がった。
  カーネル引数は書き込んだ loader entry のもの
  （`initrd=\initramfs.gz console=ttyAMA0 sarachi.net`）。

### 設計上の要点

**三つとも「ファイルシステムの像」として配る。** ESP も回復領域も root も同じ
扱いにすると、回復環境に mkfs を持ち込まずに済む。OS ごとに mkfs.vfat /
mkfs.ext4 / newfs / newfs_msdos を揃える必要がなくなり、検証して書けばよい。

**カーネルと initramfs は ESP に置く。** systemd-boot は FAT しか読めないため。
回復領域にはファームウェアを置く。initramfs に抱えると RAM に常駐する分だけ
効くが、カーネルは必要になった時に読むのでディスクで足りる。役割分担としては
WinRE（ブートマネージャのエントリは ESP、実体は回復領域）と同じ。

**ESP と回復領域は暗号化しない。** そこに秘密は入らないし、回復領域を
暗号化すると鍵を失った機体が自分で回復できなくなり、回復環境の目的そのものと
衝突する。

**署名の検証は全てディスクに触る前に済ませる。** 切ってから偽物だと分かっても
遅い。取り返しのつかない操作は、取り返しのつく検証を全て終えてから。

---

（以下は途中経過）回復環境が UEFI から RAM 上に起動し、**実際にディスクを切るところまで通った**。

```
UEFI firmware → systemd-boot → カーネル → initramfs(RAM) → /init
  → モジュール読み込み → sysfs でディスク認識 → 配置を計算 → GPT を書く
  → 報告して poweroff
```

外から確かめた証拠（32GiB の空イメージに対して）:

| 位置 | 内容 |
|---|---|
| offset 446 | `ee` = GPT protective、開始 LBA 1、サイズ `0x03FFFFFF` = 67,108,863 セクタ |
| offset 510 | `55 aa` |
| LBA 1 | `EFI PART` |
| 最終 LBA | `EFI PART`（予備ヘッダ） |

疎ファイルの実使用量が 64K であることも証拠になる。GPT の領域だけが書かれている。

initramfs は **1.75 MiB**（busybox 643KB + モジュール 934KB + 実行体 517KB）。

実装済みのクレート:

| | |
|---|---|
| `crates/disk` | GPT の配置と書き出し。CRC-32 も自前 |
| `crates/order` | 署名付き消去命令。Windows の三段に対応 |
| `crates/image` | 署名付きマニフェストと、書き込み前に検証する取り込み |
| `crates/recovery` | 回復環境で走る実行体 |

### BSD の壁は OpenSSL ではなかった（2026-09-17）

FreeBSD 15.1-RELEASE-p3 / aarch64 で `libhimmelblau` を建てたところ、
**落ちたのは LibreSSL ではなく `libkrimes`** だった。

```
error[E0425]: cannot find value `res` in this scope
   --> libkrimes-0.1.0/src/cldap.rs:133:12
```

`get_domainname()` が `res` を macOS と Linux の `cfg` でしか束縛しておらず、
BSD ではどちらの枝にも当たらない。**二行の当て物で通る。**

| 系統 | libc のモジュール | 第二引数 |
|---|---|---|
| Apple | `unix/bsd/apple` | `c_int` |
| freebsdlike（FreeBSD, DragonFly） | `unix/bsd/freebsdlike` | `c_int` |
| netbsdlike（NetBSD, OpenBSD） | `unix/bsd/netbsdlike` | `size_t` |
| linux_like | `unix/linux_like` | `size_t` |

当てた結果、FreeBSD arm64 で `libhimmelblau` 0.8.41 まで通り、
`ELF 64-bit LSB pie executable, ARM aarch64, for FreeBSD 15.1` が出来た。
当て物は `patches/` に置いてある。**まだ上流へは送っていない。**

### LibreSSL 問題は OpenBSD だけに絞られた（2026-09-17、前言を訂正）

DESIGN.md に「NetBSD と OpenBSD は LibreSSL」と書いていたが、**これは誤り**。

- **NetBSD の base は OpenSSL**（`crypto(7)` と base の libcrypto）。
  LibreSSL は pkgsrc の選択肢として在るだけで、既定ではない。
- **LibreSSL を base に持つのは OpenBSD だけ。**

そして OpenBSD 7.9 が載せているのは **LibreSSL 4.3.0**。先に測ったとおり
`openssl-sys 0.9.117` の対応上限は `libressl430` ＝ ちょうど 4.3 系なので、
**対応範囲の最上端で収まっている**。

| | base の暗号ライブラリ | 見込み |
|---|---|---|
| FreeBSD / GhostBSD | OpenSSL | **実証済み**（当て物を入れて通った） |
| DragonFly | **LibreSSL 3.6.1** | 実機で確認。ただし rust が古く `libhimmelblau` は建たない |
| NetBSD | OpenSSL | 問題は無いはず。未検証 |
| **OpenBSD** | **LibreSSL 4.3.0** | 対応範囲の上端。**通るはずだが要検証** |

OpenBSD が LibreSSL 4.4 へ進み、`openssl-sys` が追随する前だと落ちる。
ここは追いかける必要がある。

### libhimmelblau を pkgsrc パッケージにした（2026-09-18）

NetBSD 11.0/amd64 の実機で `/usr/pkgsrc/zakinko/libhimmelblau` を起こし、
白紙から 23 分で通るところまで確かめた。`pkglint` は Looks fine。

**なぜ pkgsrc が筋の良い相手だったか。** `libhimmelblau` は Rust のライブラリ
だが `crate-type = ["rlib", "cdylib"]` を持ち、`cargo cbuild`（cargo-c）で
**C の共有ライブラリとして**出る。cbindgen が C ヘッダを生成し、pkg-config
ファイルも付く。Rust からしか使えない物なら pkgsrc に入れる意味は薄いが、
これは言語を問わずリンクできる。

| 確かめたこと | 結果 |
|---|---|
| 白紙からのビルド | 23 分、`make: 0` |
| 成果物 | `.so` 17MB / `.a` 71MB / ヘッダ 72KB / `.pc` |
| バイナリパッケージ | 23MB、6 ファイル |
| **C からリンクして実行** | **通った**（`ldd`: `-lhimmelblau.0 => /usr/pkg/lib/libhimmelblau.so.0`） |

**副産物: `libhimmelblau` が NetBSD 11.0 実機で建つことが確定した。**
「cross では原理的に検証できない、実機が要る」としていた項目の一つ。

### 当て物を pkgsrc の中で運べる

`libkrimes` の当て物は、`patches/` では当たらない。crate は WRKSRC ではなく
`${WRKDIR}/vendor/` に展開されるため。`pre-configure` で当てれば通る。

```make
pre-configure:
	cd ${CARGO_VENDOR_DIR}/${LIBKRIMES} &&				\
	${PATCH} -f -p0 < ${FILESDIR}/libkrimes-bsd-getdomainname.patch
```

**成立の根拠**は `cargo.mk` が書く `.cargo-checksum.json` が
`{"package":"...","files":{}}` と個別ファイルのハッシュを持たないこと。
vendor した crate を書き換えても cargo は拒まない。

これで**上流が当て物を取るかどうかと独立に、一箇所で抱えたまま配れる**。

### 「全 OS を pkgsrc ベースに」の評価

`mk/platform` は AIX Cygwin Darwin DragonFly FreeBSD FreeMiNT HPUX Haiku IRIX
**Linux** MidnightBSD Minix NetBSD OSF1 OpenBSD QNX SCO_SV SunOS UnixWare を
持つ。**本プロジェクトの 12 標的が全部入っている。**

得られるもの:

- パッケージ形式の分裂（rpm×3 / deb×3 / apk / ports×2 / pkgsrc / dports）が 1 つになる
- 当て物を一箇所で抱えられる
- **pkgsrc 自身の `lang/rust` を使うので、OS が配る rust の版に左右されない。**
  DragonFly で `libhimmelblau` が建たなかったのは配布 rust が 1.85 と古いため
  だったが、pkgsrc 経由なら起きない

代償:

- **Linux で一台ずつ bootstrap するのは重すぎる。** ただし (OS, arch) ごとに
  一度ビルドしてバイナリパッケージを配れば実用になる。署名した成果物を配る
  という本プロジェクトの設計とも相性が良い
- pkgsrc は `/usr/pkg` 配下に入れる。Linux の常駐デーモンは `/usr/sbin` と
  `/usr/lib/systemd` を期待するので、そこは OS ごとの結合部として残る
- **回復環境は対象外。** 自己完結した initramfs なのでパッケージにする必要がない。
  pkgsrc が効くのはエージェント側

### DragonFly 実機での結果（2026-09-18）— 前言を二つ訂正

`zakinko/netbsd-ci-images` の image で実機の DragonFly 6.4.2-RELEASE を立て、
その上で確かめた。

**訂正 1: DragonFly は LibreSSL。** 「freebsdlike だから base は OpenSSL」と
書いていたが誤り。実機の `openssl version` は **LibreSSL 3.6.1** だった。
LibreSSL を base に持つのは OpenBSD だけ、という前の記述も誤りになる。

| | base の暗号ライブラリ | 確かめ方 |
|---|---|---|
| FreeBSD | OpenSSL | 実機（`libhimmelblau` が通った） |
| **DragonFly** | **LibreSSL 3.6.1** | **実機で確認** |
| NetBSD | OpenSSL | 未確認（man と base の libcrypto から） |
| OpenBSD | LibreSSL 4.3.0 | リリースノート |

**訂正 2: DragonFly では `libhimmelblau` が建たない。ただし当て物のせいではない。**
`yoke-derive 0.8.3` が `str::from_utf8` を使っているが、DragonFly が配っている
**rustc が 1.85.1（2025-03）と古く**、その関連関数をまだ持たない。

```
error[E0599]: no function or associated item named `from_utf8` found for type `str`
   --> yoke-derive-0.8.3/src/lib.rs:202:32
```

つまり**移植性の問題ではなく、パッケージの Rust が現行の crate 生態系に
追いついていない**という別種の壁。`libhimmelblau` を DragonFly で動かすには、
rust を自前で新しく入れるか、上流の更新を待つことになる。

**収穫: 素の `libkrimes` が実機の DragonFly でも `E0425` で落ちることを確認した。**
cross での再現より強い証拠になる（`patches/README.md` の表を更新済み）。

なお image の root は 1750M しかなく、`rust` が依存込みで 1 GiB 要るため入らない。
`runvm.sh` の `EXTRAARGS` で作業用ディスクを足し、`/usr/local` をそこへ移して回避した。

### cross では答えの出ない問いがある（2026-09-17）

`libkrimes` は純 Rust なので、nightly の `-Z build-std` を使えば tier 3 の
DragonFly と OpenBSD 向けにも手元で建てられた。当て物の検査はこれで足りた。

**しかし `libhimmelblau` 本体は同じ手では確かめられない。** C のライブラリを
要るためで、cross では標的側のそれが無い。

| 標的 | 落ちた場所 |
|---|---|
| x86_64-unknown-dragonfly | `openssl-sys` — 標的の OpenSSL が見つからない |
| x86_64-unknown-openbsd | 同上 |
| x86_64-unknown-netbsd | **`aws-lc-sys`** — rustls の暗号バックエンドが標的ごとに違う |

NetBSD で `aws-lc-sys` が出てくるのは注意すべき点で、こちらも C のライブラリ
なので、それ自体が移植性の関門になり得る。

→ **OpenBSD の LibreSSL 問題は cross では原理的に検証できない。実機が要る。**
これは「箱が無い」とは別の問題で、箱はある（`vmactions/openbsd-vm` の 7.9、
または `zakinko/netbsd-ci-images` の `build-openbsd-image.sh`）。
CI を足すかどうかがユーザの判断待ちなので、そこで止まっている。

### 計測の罠（この夜に踏んだもの）

- `cargo build 2>&1 | tail -30` の終了ステータスは `tail` のもので、
  **cargo の成否にならない**。一度これで「成功」と誤報した。
  成果物の存在で判定すること。
- FreeBSD には python3 が入っていない。当て物は sed で当てた。
- lima の `vm-type=vz`（Virtualization.framework）は **Linux 専用**で、
  FreeBSD は起動しない。`--vm-type=qemu` を使う。無言で失敗し、
  インスタンスのディレクトリにディスクイメージが出来ないので気づきにくい。

### 取得から書き戻しまでの実地試験（2026-09-17）

qemu の回復環境から、Mac 上の HTTP サーバへ実際に取りに行かせた。

**正常な場合**

```
マニフェスト: http://10.0.2.2:8088/test.manifest
  198 バイト取得
  署名: 検証を通った
  名前: test.img / 20 MiB（3 チャンク x 8 MiB）
/dev/vdb に書いた（3 チャンク / 20 MiB）
```

書かれた 20MiB は元イメージと **SHA-256 が一致**し、20MiB 以降は 0 のままだった。

**改竄された場合**（2 番目のチャンクの中の 1 バイトを反転、マニフェストは元のまま）

```
Error: チャンク 1 のハッシュが一致しない。書かずに中止した
  期待 7680eb70...
  実際 0ff35385...
```

実際に書かれた量は **ちょうど 8.00 MiB = 1 チャンク**。検証を通った 1 個目だけが
書かれ、**壊れた 2 個目は 1 バイトも書かれていない**。設計の要としていた性質が、
走っている回復環境と実際の HTTP 越しに満たされた。

initramfs は 2.8 MiB（busybox 643KB + モジュール + 実行体 2.4MB）。
実行体が太ったのは rustls と証明書を抱えたため。OpenSSL を持ち込まない代償で、
回復環境全体では十分小さい。

## 暗号層と root の型（2026-09-21）

### 文書と実装が食い違っていたので正す

「OS 抽象層を置く」と書いてあったが、**コードには抽象が無く実装が一つ
（cryptsetup）あるだけだった**。trait を入れて cryptsetup をその一実装に
降ろした。

```rust
pub trait Crypt {
    fn format(&self, device: &Path, key: &[u8]) -> Result<()>;
    fn open(&self, device: &Path, key: &[u8], name: &str) -> Result<PathBuf>;
    fn close(&self, name: &str) -> Result<()>;
    fn erase(&self, device: &Path) -> Result<()>;
    fn is_container(&self, device: &Path) -> bool;
}
```

`open` が**開いた先のパスを返す**のは、それが OS ごとに違うため。LUKS は
`/dev/mapper/<name>`、geli は `<device>.eli`、cgd は `/dev/cgd<N>`、OpenBSD は
新しく現れる `/dev/sd<N>`。呼び出し側では組み立てられない。

**実装していない OS は断る。** 黙って別の手を使わない。消去は取り返しが
つかないので、確かめていない経路は通さない。試験で固定してある。

### 型 GUID は実行時に決める

**`cfg!(target_os)` では決められない。** 回復環境は常に Linux だが、導入する
先は FreeBSD や NetBSD でありうる。`RootKind` として実行時の引数にした
（`--root-kind`）。値は推測ではなく NetBSD src の `sys/sys/disklabel_gpt.h`
から取った。

| RootKind | GUID |
|---|---|
| LinuxLuks | `ca7d7ccb-63ed-4c53-861c-1742536059cc` |
| FreeBsdZfs | `516e7cba-6ecf-11d6-8ff8-00022d09712b` |
| FreeBsdUfs | `516e7cb6-6ecf-11d6-8ff8-00022d09712b` |
| NetBsdCgd | `2db519ec-b10f-11dc-b99b-0019d1879648` |
| NetBsdFfs | `49f48d5a-b10e-11dc-b99b-0019d1879648` |
| OpenBsdData | `824cc7a0-36a8-11e3-890a-952519ad3f61` |

**型から暗号化が分かるのは LUKS と cgd だけ。** geli と softraid は下に敷く
だけで型を変えないので、消す前の判断に型を使えない。

ESP と回復領域の型は載せる物で変わらない。**起動物の置き場は全 OS で同じ形に
でき、分岐するのは root だけ**というのが、この節の要点になる。

## NetBSD の root-on-ZFS（2026-09-21）

NetBSD でも root-on-ZFS はできる（sysinst は未対応だが、**我々には効かない。
回復環境そのものが導入器だから**）。

仕組みは「FFS から起動して pivot する」形で、**ブートローダは ZFS を読まない**
（`/usr/mdec` に `bootxx_zfs` が無いのはそのため）。

```
ブートローダ → FFS から カーネル + solaris/zfs モジュール + ZFS root ramdisk
ramdisk     → rpool を import → rpool/ROOT を /altroot へ → chroot
```

NetBSD 11.0/amd64 は `ramdisk-zfsroot.fs` を同梱する。実機で `zfs.kmod` と
`solaris.kmod` が base にあることは確認した。

**これは我々の構造とそのまま噛み合う。** 要求は「ブートローダが読める領域に
起動物を置く」ことで、それは ESP にカーネルと initramfs を置く今の形と同じ。
パーティションを増やす必要がない。発想も同じで、ブートローダに賢さを求めず
RAM 上の小さな環境に任せる。

三つの形が揃った:

| | ブートローダ | root-on-ZFS |
|---|---|---|
| FreeBSD / GhostBSD | ZFS を読める | 直接 |
| NetBSD | 読めない | FFS から起動して ramdisk で pivot |
| DragonFly / OpenBSD | — | ZFS が無い |

なお cgd と root のファイルシステムは独立なので、cgd の上に ZFS を載せられる。
「ブロック層の暗号で全標的を覆う」方針は NetBSD でも崩れない。

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
DragonFly と armhf は手元の x86 機か実 Pi にまとめて当てればよい。
→ **arm64 先行で 9 割方が進み、残り 3 つを最後に一括**。

### amd64 側は vmactions で埋まる（2026-09-17 実査）

上で「x86 機か実機が要る」と書いた穴は、GitHub Actions の `vmactions/*` で
埋まる。**必要な 5 つの BSD と Alpine の amd64 イメージが全て実在する。**

| | amd64 | arm64 |
|---|---|---|
| freebsd 15.1 | `freebsd-15.1.qcow2.zst` | あり |
| netbsd 11.0 | `netbsd-11.0.qcow2.zst` | あり |
| openbsd 7.9 | `openbsd-7.9.qcow2.zst` | あり |
| dragonflybsd 6.4.2 | `dragonflybsd-6.4.2.qcow2.zst` | **無し** |
| ghostbsd 26.1 | `ghostbsd-26.1.qcow2.zst` | **無し** |
| alpine 3.24 | `alpine-3.24.qcow2.zst` | あり |

DragonFly と GhostBSD に arm64 が無いのは、それぞれの公式情報から出した
「amd64 のみ」という結論とイメージ側でも一致している。

**そして amd64 ゲストは KVM で走る。** action の `index.js` を読んで確かめた:

- 712-713 行で `/dev/kvm` が在れば `chmod 666` している
- 448 行のコメントが「x86_64 runner では x86_64/amd64 ゲストだけが KVM で走る」
- `isSlowEmulatedArch(arch)` は `!!arch && arch !== 'x86_64' && arch !== 'amd64'` で、
  使われているのは 1396 / 1507 / 1802 の 3 箇所だけ。488 行のコメントどおり
  rsync のタイムアウト調整であって、**加速器の選択はしていない**

つまり「vmactions は TCG だから遅い」は、ホストと arch が違うゲスト
（aarch64, riscv64, sparc64, ...）にしか当てはまらない。

確認は記憶ではなく repo に訊く。conf は `vmactions/<os>-vm/contents/conf`、
image は `anyvm-org/<os>-builder` の release asset で、**タグには `v` が付く**
（`BUILDER_VERSION` の値をそのまま貼ると 404 が返り、それが「この release は
無い」に見えてしまう）。`vmactions/<os>-builder` は古いので見ないこと。

**ただし CI は自動で足さない。** このリポジトリはまだ remote が無く、
CI を足すかどうかはユーザの判断。`vmactions/*` は起動と導入で 8〜12 分かかる
高い部類なので、入れるなら既定ブランチへの push と `workflow_dispatch` と
schedule に限り、PR の push ごとには回さないこと。

### 既知の罠

- **NetBSD を Mac arm の brew 版 qemu で動かすには当て物が要る**（ユーザ実体験）。
  内容は**要記録** — 次に NetBSD VM を立てる時に再発するので、詳細をここに書き残すこと。
- **Alpine の aarch64 カーネルは EFI zboot 形式**（`MZ..zimg` で始まる PE32+）。
  qemu の `-kernel` による直接起動では読めず、**何の出力も出さずに固まる**ので
  原因が分かりにくい。EFI 経由で起動すること（設計上もそれが本来の経路）。
- **initramfs の `/bin/sh` はビルド時に張る。** `/init` の shebang が解決される
  時点で既に無いといけない。`busybox --install` は init の中で走るので間に合わず、
  カーネルは `/init` を ENOENT で見失い "No working init found" で panic する。
  症状（panic）と原因（symlink 不在）が遠いので、一度踏むと分かりにくい。
- **qemu の出力を `tail` に繋ぐと何も見えない。** バッファリングで EOF まで出ない。
  ファイルへ落として別途読むこと。

### 実測値（2026-09-16、Alpine 3.23.4 / aarch64 / kernel 6.18.22-0-virt）

| | 大きさ |
|---|---|
| initramfs（busybox-static のみ） | 643 KB |
| initramfs（+ 選んだモジュール 6 種） | **1.5 MB** |
| カーネル（vmlinuz-virt） | 9.8 MB |
| 合計 | **約 11 MB**（回復パーティション 2GB の 0.5%） |
| 参考: モジュールを全部入れた場合 | 22.7 MB |

モジュールは明示列挙（virtio_blk, virtio_net, nvme, dm-crypt, ext4, vfat）。
依存はビルド時に `modprobe --show-depends` で解決して順序を固定し、実行時は
insmod を並べるだけにしてある。initramfs に depmod も modules.dep も要らず、
挙動も決定的になる。

**ただしこれは VM 用の `virt` カーネルでの値**。実機（ノート、Pi）では `lts`
カーネルと `linux-firmware` が要る。

### ファームウェアの実測（2026-09-17）

Alpine は `linux-firmware` を 112 のサブパッケージに割っているので絞り込める。

| 範囲 | 大きさ |
|---|---|
| 丸ごと (`linux-firmware`) | **727 MiB** |
| 無線14 + 有線6 に絞る | **287 MiB**（39%） |
| ノート/Pi 想定に絞る（サーバNIC・Marvell・TI を落とす） | **188 MiB** |

絞った 188 MiB の内訳は `intel` が突出。ただし決定的なのは次の事実:

**iwlwifi は 330 版が入っているのに、一台が実際に読むのは 1〜2 ファイルで、
各 1 MiB 未満。**

### 決定: ファームウェアは initramfs ではなく回復パーティションに置く

数字が示しているのは、絞り込みの上手下手ではなく**置き場所が間違っている**こと。
カーネルはファームウェアを必要になった時に読むので、全部を RAM に抱える必要がない。

- **initramfs は約 2MB のまま RAM 常駐**。速く、2GB の Pi でも軽い
- **回復パーティション 2GB に 188MB のファームウェア一式**。十分収まる
- 起動時に read-only でマウントし、ハードウェアに応じた数 MB だけが読まれる

第 3 段（紛失・盗難用の破壊）で回復領域ごと消す場合も成立する。
**ファームウェアは一度デバイスに読み込まれればファイルは不要**なので、
ネットワークを上げてからアンマウントして消せば、無線は繋がったまま消去できる。

これは WinRE とも整合する。WinRE も回復パーティションに住み、第 1 段・第 2 段では
その領域が残る。消えるのは第 3 段だけで、その時はもう再インストールしない。

## 当面の進め方

1. **`libhimmelblau` 単体**を Alpine aarch64 と FreeBSD arm64 でビルドし、
   どこで壊れるかを実測する（Himmelblau 全体ではない。上記の決定を参照）。
2. 並行して、MS 非依存の `WipeOrder`（署名付きコマンド）とエージェント骨格を Rust で起こす。
3. crypto-erase を Linux / FreeBSD の 2 つで先に通し、OS 抽象層の形を確定させてから
   残り 4 BSD に広げる。
