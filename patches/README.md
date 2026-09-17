# 上流へ出す候補の当て物

ここに置いてあるものは **まだどこにも送っていない。そして今は送れる状態にない。**

送る前に必要なことは 手順 の「Pull requests」にある。特に
**diff が名指しする platform は一つずつ実際に動かす**こと。動かしていない行を
「Not tested: X」と書いて出すのは駄目で、X を diff から外して逃げるのも駄目。

## libkrimes-0.1.0-bsd-getdomainname.diff

`libkrimes` は `libhimmelblau` の依存で、`libhimmelblau` は本プロジェクトが
Entra ID と Intune を相手にするために使う。その `libkrimes` が **BSD で
ビルドできない**。

`src/cldap.rs` の `get_domainname()` が `res` を macOS と Linux の二つの
`cfg` でしか束縛しておらず、どちらにも当たらない標的では未定義参照になる。

```
error[E0425]: cannot find value `res` in this scope
   --> libkrimes-0.1.0/src/cldap.rs:133:12
    |
133 |         if res == -1 {
    |            ^^^ not found in this scope
```

型は libc クレートの宣言に合わせてある。推測ではなくソースを見て決めた。

| 系統 | libc のモジュール | 第二引数 |
|---|---|---|
| Apple | `unix/bsd/apple/mod.rs` | `c_int` |
| freebsdlike（FreeBSD, DragonFly） | `unix/bsd/freebsdlike/mod.rs` | `c_int` |
| netbsdlike（NetBSD, OpenBSD） | `unix/bsd/netbsdlike/mod.rs` | `size_t` |
| linux_like | `unix/linux_like/mod.rs` | `size_t` |

### diff が名指しする platform と、その検査の状態

この diff は `freebsd` `dragonfly` `netbsd` `openbsd` の四つを名指ししている。
**四つとも動かすまで送らない。**

| platform | 当てる前に落ちるか | 当てた後に建つか | 走るか |
|---|---|---|---|
| FreeBSD aarch64 | 確認済（実機 15.1-RELEASE-p3） | **確認済**（`libhimmelblau` 0.8.41 まで通り実行体が出来た） | 未 |
| FreeBSD x86_64 | — | **確認済**（`cargo build --target x86_64-unknown-freebsd`） | 未 |
| NetBSD x86_64 | **確認済**（`error[E0425]`） | **確認済**（`cargo build --target x86_64-unknown-netbsd`） | 未 |
| DragonFly x86_64 | 未 | **未** | 未 |
| OpenBSD x86_64 | 未 | **未** | 未 |

「建った」と「走った」は別の検査。上の表の「走るか」が全部未なのは、
`getdomainname` が実際に正しい値を返すところまでは見ていないという意味。

### 箱の在り処（確認済み、2026-09-17）

「箱が無い」は成立しない。三つとも在る。

- **DragonFly**: `zakinko/netbsd-ci-images` の release `images` に
  `dragonfly-6.4.2-x86_64.qcow2`（`.qemu` に繋ぎ方あり）
- **OpenBSD**: `vmactions/openbsd-vm` の `7.9.conf`、image は
  `anyvm-org/openbsd-builder` の `v2.1.0` に `openbsd-7.9.qcow2.zst`
- OpenBSD は `zakinko/netbsd-ci-images` の `build-openbsd-image.sh` でも作れる

### 送る前にやること

1. DragonFly x86_64 で当てる前・当てた後の両方を動かす
2. OpenBSD x86_64 で同じことをする
3. できれば各 platform で `getdomainname` が正しい値を返すところまで見る
4. 上流の最新版でまだ再現するか確かめる（試したのは crates.io の 0.1.0）
5. 本文を英語と日本語の両方で書き、ユーザーに見せる
