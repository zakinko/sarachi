# 上流へ出す候補の当て物

ここに置いてあるものは **まだどこにも送っていない**。送る前に必要なことは
手順 の「Pull requests」に書いてあるとおりで、本文を英語と
日本語の両方で示し、こちらが読んでから `gh pr create` なり mail なりを走らせる。

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

つまり freebsdlike は既存の macOS の枝に、netbsdlike は既存の Linux の枝に
そのまま相乗りできる。新しい分岐は要らない。

### 確かめたこと

- FreeBSD 15.1-RELEASE-p3 / aarch64 で、当てる前は上記のエラーで落ち、
  当てた後は `libhimmelblau` 0.8.41 まで通って実行体が出来る
  （`ELF 64-bit LSB pie executable, ARM aarch64, for FreeBSD 15.1`）。

### 確かめていないこと

- NetBSD、OpenBSD、DragonFly では**試していない**。型は libc の宣言から
  導いたもので、実際に建ててはいない。
- 出来た実行体が Kerberos として正しく動くかは見ていない。ビルドが通った
  ことと、動くことは別の話。
- 上流の最新版でまだ再現するか（試したのは crates.io の 0.1.0）。
