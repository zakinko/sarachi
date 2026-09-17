# 上流へ出す候補の当て物

ここに置いてあるものは **まだどこにも送っていない。** 送る前に必要なことは
手順 の「Pull requests」にある。

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

freebsdlike は既存の macOS の枝に、netbsdlike は既存の Linux の枝にそのまま
相乗りできるので、新しい分岐は要らない。

## 検査（2026-09-17）

diff が名指しする platform は `freebsd` `dragonfly` `netbsd` `openbsd` の四つ。
**四つとも当てる前に落ちることと、当てた後に建つことを確かめた。**

| platform | 当てる前 | 当てた後 | 走らせた | 手段 |
|---|---|---|---|---|
| FreeBSD aarch64 | `E0425` | 建つ | **通った** | 実機 15.1-RELEASE-p3 |
| FreeBSD x86_64 | — | 建つ | — | cross（rustup の std） |
| NetBSD x86_64 | `E0425` | 建つ | — | cross（rustup の std） |
| DragonFly x86_64 | `E0425` | 建つ | — | cross（`-Z build-std`） |
| OpenBSD x86_64 | `E0425` | 建つ | — | cross（`-Z build-std`） |
| macOS aarch64 | （既存の枝） | — | **通った** | 実機 |
| Linux aarch64 musl | （既存の枝） | — | **通った** | 実機（Alpine 3.23） |

「建つ」は `cargo build` が成果物を出すところまで（`check` ではない）。
FreeBSD aarch64 では `libkrimes` だけでなく **`libhimmelblau` 0.8.41 まで通り、
実行体が出来ている**（`ELF 64-bit LSB pie executable, ARM aarch64, for FreeBSD 15.1`）。

### 走らせた範囲について

**この当て物が持つ二つの枝は、どちらも実機で走っている。**
`c_int` の枝は FreeBSD aarch64 で、`size_t` の枝は Linux aarch64 で、
`getdomainname` が戻り値 0 を返し、値が取れるところまで確かめた
（macOS でも同じく通る）。

NetBSD・DragonFly・OpenBSD で走らせていないのは、それらが選ぶ枝が既に
走っている枝と同じものだから。OS ごとに違うのは libc の宣言のほうで、
そちらはコンパイル時に照合される。**とはいえ「建った」と「走った」は別の
検査なので、表では分けてある。**

### 手元で cross できた理由

- FreeBSD と NetBSD の x86_64 は `rustup` が std を配っている
- DragonFly と OpenBSD は tier 3 で std が無いが、nightly の
  `-Z build-std=core,alloc,std,panic_abort` で std ごと組めば当てられる

いずれも本物の標的に対する本物のコンパイルなので、欠陥（コンパイル時の
未定義参照）に対しては直接効く検査になる。

## 残っていること

1. 上流の最新版でまだ再現するか（試したのは crates.io の 0.1.0）
2. 本文を英語と日本語の両方で書き、ユーザーに見せる
