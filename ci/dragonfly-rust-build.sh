#!/bin/sh
# DragonFly 向けの rustc / rust-std / cargo を建てて dist/ に置く。
#
#   ci/dragonfly-rust-build.sh <version> <llvm package> [bootstrap url]
#
# bootstrap を省くと pkg の rust (1.85.1) を種にする。二段目からは前の段が
# 出した tarball の置き場を渡す。rustc は一つ前の版でしか建たないので、
# 1.85.1 → 1.86 → 1.87 → 1.88 と順に積むことになる。
set -e

VERSION=$1
LLVMPKG=$2
BOOTSTRAP_URL=$3
BOOTSTRAP_VER=$4

[ -n "$VERSION" ] || { echo "版が要る" >&2; exit 1; }

# LLVM の版は rustc の版が決める。手で渡させると間違える。実際に一度
# 間違えた: 1.86.0 に llvm20 を渡したら、同梱の lld が
#
#   lld/Common/DWARF.cpp:97: error: no matching function for call to
#     'getFileLineInfoForAddress' (候補は 5 引数、こちらは 4 引数)
#
# で 22 分かけて落ちた。lld の source は同梱 LLVM の API に合わせて書かれて
# いて、rustc 本体と違って版差の #if を持っていない。
#
# DPorts が配る binary がちょうど合う版を持っている。
llvm_for() {
	case $1 in
	1.86.*) echo llvm19 ;;  # 同梱 19.1.7  DPorts llvm19-19.1.7_1
	1.87.*) echo llvm20 ;;  # 同梱 20.1.1  DPorts llvm20-20.1.1
	1.88.*) echo llvm20 ;;  # 同梱 20.1.5  DPorts は 20.1.1（major/minor 一致）
	*)
		echo "$1 に合う LLVM が表に無い。" >&2
		echo "rust-lang/llvm-project の cmake/Modules/LLVMVersion.cmake で" >&2
		echo "同梱の版を見て、表に足すこと。" >&2
		return 1
		;;
	esac
}

[ -n "$LLVMPKG" ] || LLVMPKG=$(llvm_for "$VERSION") || exit 1
echo "### rustc $VERSION には $LLVMPKG を使う"

# LLVM はここで入れる。prepare で決め打ちにはできない。版によって変わる。
#
# 合う版だけを置く。rust の lld を建てる step は [target.*] の llvm-config を
# 使わず CMake の find_package で自分で探すので、違う版が転がっていると
# そちらを拾う。llvm19 と llvm20 を両方入れて 1.86.0 を建てたら、
# config.toml には llvm-config19 と書いてあるのに lld の compile だけが
# -I/usr/local/llvm20/include を拾って落ちた。今は lld を建てないように
# してあるが、置かないのが一番確実。
#
# zstd も要る。静的に link すると LLVM が要求する外部のライブラリが
# そのまま link 行に出てくるが、base に zstd は無い。
#
#   /usr/libexec/binutils234/elf/ld.bfd: cannot find -lzstd
#
# 動的 link だった間は libLLVM.so の側が抱えていたので表に出なかった。
pkg install -y "$LLVMPKG" zstd libssh2

# 入った物が在ることをここで確かめる。無いまま建て始めると、気づくのは
# 20 分以上あとの link 段階になる。
for lib in libzstd.a libzstd.so libssh2.so; do
	[ -e "/usr/local/lib/$lib" ] || { echo "/usr/local/lib/$lib が無い" >&2; exit 1; }
done
echo "  zstd: $(ls /usr/local/lib/libzstd.* | tr '\n' ' ')"
if [ -n "$BOOTSTRAP_URL" ] && [ -z "$BOOTSTRAP_VER" ]; then
	echo "置き場を渡すなら、種の版も要る" >&2
	exit 1
fi

# 手元の directory を渡された場合は絶対に直す。この下で作業木へ cd するので、
# 相対のままだと解決先が変わる。
if [ -n "$BOOTSTRAP_URL" ] && [ -d "$BOOTSTRAP_URL" ]; then
	BOOTSTRAP_URL=$(cd "$BOOTSTRAP_URL" && pwd)
fi

TRIPLE=x86_64-unknown-dragonfly
OUT=$(pwd)/dist
# / は 134G あるが、/build は build のために切られた別の partition なので
# そちらを使う。45G あり、rustc には十分。
WRK=/build/rust
rm -rf "$WRK"; mkdir -p "$WRK" "$OUT"

say() { echo; echo "### $* ($(date '+%H:%M:%S'))"; }

say "llvm-config を探す"
# DPorts の llvm は版つきの名前で入る (llvm-config20 など)。名前を決め打ちに
# すると package の付け方が変わったときに黙って同梱 LLVM を建て始めるので、
# 見つからなければここで止める。
SUFFIX=$(echo "$LLVMPKG" | tr -dc '0-9')
LLVM_CONFIG=""
for c in "/usr/local/bin/llvm-config${SUFFIX}" "/usr/local/llvm${SUFFIX}/bin/llvm-config"; do
	[ -x "$c" ] && { LLVM_CONFIG=$c; break; }
done
[ -n "$LLVM_CONFIG" ] || { echo "llvm-config が見つからない ($LLVMPKG)" >&2; ls /usr/local/bin | grep -i llvm >&2; exit 1; }
echo "  $LLVM_CONFIG ($("$LLVM_CONFIG" --version))"

say "種にする rustc を決める"
if [ -n "$BOOTSTRAP_URL" ]; then
	# 前の段が出した tarball を入れて、それを種にする。置き場には
	# rustc- rust-std- cargo- の三つが <part>-<ver>-<triple>.tar.xz の名前で
	# 並んでいる前提。mneumann 氏が leaf.dragonflybsd.org で配っている形に
	# 合わせてあるので、将来そちらへ戻す時にも同じ名前で済む。
	BOOT=$WRK/bootstrap
	mkdir -p "$BOOT" "$WRK/boottar"
	cd "$WRK/boottar"
	for part in rustc rust-std cargo; do
		f="${part}-${BOOTSTRAP_VER}-${TRIPLE}.tar.xz"
		# 置き場は URL でも手元の directory でもよい。段を繋ぐ間は前の段の
		# artifact を手元に降ろして渡す。公開するのは三段そろってから。
		if [ -d "$BOOTSTRAP_URL" ]; then
			cp "$BOOTSTRAP_URL/$f" .
		else
			curl -sfL -O "${BOOTSTRAP_URL%/}/${f}"
		fi
		tar xf "$f"
		(cd "${f%.tar.xz}" && ./install.sh --prefix="$BOOT" --disable-ldconfig)
	done
	cd "$WRK"
else
	BOOT=/usr/local
	echo "  pkg の rust を使う: $(rustc --version)"
fi
RUSTC_BIN=$BOOT/bin/rustc
CARGO_BIN=$BOOT/bin/cargo
[ -x "$RUSTC_BIN" ] || { echo "種の rustc が無い: $RUSTC_BIN" >&2; exit 1; }
echo "  種: $("$RUSTC_BIN" --version)"

say "rustc ${VERSION} の source を取る"
cd "$WRK"
SRC=rustc-${VERSION}-src.tar.xz
curl -sfL -O "https://static.rust-lang.org/dist/${SRC}"
curl -sfL -O "https://static.rust-lang.org/dist/${SRC}.sha256"
# 配布物の取り違えは build の何時間も先で妙な形で出るので、ここで止める。
# DragonFly の sha256 は FreeBSD と違って -c を持たない (getopt は
# "hb:e:pqrs:tx")。-q で値を出して自分で比べる。
WANT=$(awk '{print $1}' "${SRC}.sha256")
GOT=$(sha256 -q "$SRC")
if [ "$WANT" != "$GOT" ]; then
	echo "sha256 が合わない: 期待 $WANT 実際 $GOT" >&2
	exit 1
fi
echo "  sha256 一致"
tar xf "$SRC"
cd "rustc-${VERSION}-src"

say "-fPIC を付ける cc の包みを作る"
# DragonFly は既定で PIE を作るが、-sys crate が建てる C の source には
# -fPIC が付かない。cargo の link で libssh2-sys がこうなる。
#
#   liblibssh2_sys-....rlib(agent.o): relocation R_X86_64_32 against
#     .rodata.str1.1 can not be used when making a PIE object
#
# 環境変数では届かなかった。素の CFLAGS は bootstrap が立てる
# CFLAGS_<triple を下線にした物> に負け、その名前で渡しても tool
# （cargo）を建てる経路では効かなかった。config.toml の [target.*] に
# cflags という項目は無い。
#
# cc 自体を包めば、どの経路から呼ばれても付く。cc と cxx は [target.*] が
# 受け付ける項目なので、そこから指す。
mkdir -p "$WRK/bin"
cat > "$WRK/bin/cc" <<'WRAP'
#!/bin/sh
exec /usr/bin/cc -fPIC "$@"
WRAP
cat > "$WRK/bin/c++" <<'WRAP'
#!/bin/sh
exec /usr/bin/c++ -fPIC "$@"
WRAP
chmod +x "$WRK/bin/cc" "$WRK/bin/c++"
"$WRK/bin/cc" --version | head -1 | sed 's/^/  /'

say "config.toml を書く"
# この heredoc は変数を展開させるので引用していない。つまり中身は shell に
# 読まれる。backtick を書くと command substitution として実行され、comment の
# つもりの行が静かに消える。実際にやった: 失敗した log を貼った comment の
# backtick で rustc -vV が走り、後から入れた comment では引用が閉じずに
# Syntax error になった。ここに backtick とドル記号を書かないこと。
cat > config.toml <<CONF
[llvm]
# 同梱の LLVM を建てると 4 core では二時間以上かかり、job の上限に収まらない。
# package の LLVM に向ける。
download-ci-llvm = false
# link-shared は既定の false のままにする。true にすると出来た rustc が
# libLLVM-<版>.so を要り、その package が入っている機械でしか動かない。
# 配る物としてそれは困るし、次の段で実際に困った: llvm19 に動的 link した
# 1.86 を、llvm20 しか入っていない 1.87 の run で種にしたら
#
#   error: process didn't exit successfully: .../bin/rustc -vV (exit status: 1)
#
# で止まった。静的なら種にも配布物にも、置き場の LLVM が要らない。

[build]
build = "${TRIPLE}"
host = ["${TRIPLE}"]
target = ["${TRIPLE}"]
rustc = "${RUSTC_BIN}"
cargo = "${CARGO_BIN}"
python = "python3"
docs = false
extended = true
# rustdoc も出す。次の段の bootstrap は initial_rustc の隣に rustdoc が
# ある前提で path を組み立てる（bootstrap の initial_rustdoc は
# initial_rustc.with_file_name("rustdoc")）。実際に呼ばれるかは走らせる step
# 次第だが、無くて落ちると 40 分が無駄になる。入れる方が安い。
tools = ["cargo", "rustdoc"]
vendor = true

[install]
prefix = "/usr/local"

[rust]
channel = "stable"
# lld は建てない。DragonFly は system の linker を使うので要らないうえ、
# lld の build step だけが外の LLVM を自分で探しに行って版を拾い違える。
# 要らない物のために失敗する面を持たない。
lld = false
# 配る物に debug 情報は要らない。build 時間と成果物の大きさの両方に効く。
debug = false
debug-assertions = false
# codegen-units は既定のままにする。1 にすると出来る rustc は速くなるが
# build 時間が大きく伸びる。ここで作るのは次の段の種なので、建つことと
# 六時間に収まることを採る。

[target.${TRIPLE}]
llvm-config = "${LLVM_CONFIG}"
cc = "${WRK}/bin/cc"
cxx = "${WRK}/bin/c++"
CONF
cat config.toml | sed 's/^/  /'

say "建てる"
# ld.gold では std を rustc driver に link できない。DPorts の
# Makefile.DragonFly が同じ理由で ld.bfd を指定している。
LDVER=ld.bfd
export LDVER
# pkg の物は /usr/local/lib に入るが、そこは linker の既定の探索路ではない。
# 静的 LLVM が要求する -lzstd がここで見つからず
#
#   ld.bfd: cannot find -lzstd
#
# になる。DPorts の lang/rust が外の LLVM を使うときに同じことをしている
# （PORT_LLVM_MAKE_ENV= RUSTFLAGS="-Lnative=${LOCALBASE}/lib"）。
RUSTFLAGS="-Lnative=/usr/local/lib"
export RUSTFLAGS
# bootstrap は自分の段ごとに RUSTFLAGS を組み直すので、そちらにも渡す。
RUSTFLAGS_BOOTSTRAP=$RUSTFLAGS
RUSTFLAGS_NOT_BOOTSTRAP=$RUSTFLAGS
export RUSTFLAGS_BOOTSTRAP RUSTFLAGS_NOT_BOOTSTRAP
# cc が link するときにも効かせる。
LIBRARY_PATH=/usr/local/lib
export LIBRARY_PATH
# DragonFly は既定で PIE を作るが、cc crate が建てる C の source には
# -fPIC が付かない。cargo の link で libssh2-sys がこうなる。
#
#   liblibssh2_sys-....rlib(agent.o): relocation R_X86_64_32 against
#     .rodata.str1.1 can not be used when making a PIE object
#
# 同じ物を要求する -sys crate は他にもある（libgit2 blake3 psm）ので、
# 個別にではなく CFLAGS で一度に渡す。
# DragonFly は既定で PIE を作るが、-sys crate が建てる C の source には
# -fPIC が付かない。cargo の link で libssh2-sys がこうなる。
#
#   liblibssh2_sys-....rlib(agent.o): relocation R_X86_64_32 against
#     .rodata.str1.1 can not be used when making a PIE object
#
# 素の CFLAGS では届かない。bootstrap が target ごとに CFLAGS_<triple を
# 下線にした物> を立てるので、cc crate から見てそちらが強い。config.toml の
# [target.*] には cflags という項目が無い（cc cxx ar ranlib default-linker
# linker split-debuginfo llvm-config …）ので、そこにも書けない。
#
# bootstrap はその環境変数を読んで自分の flag に継ぎ足す作りになっている
# （src/bootstrap/src/core/builder/cargo.rs）。同じ名前で渡せば通る。
# libssh2 は package の物を使い、C を建てさせない。
#
# cargo が抱える libssh2-sys は cc::Build で C を建てるが、その object に
# -fPIC が付かず、DragonFly が既定で作る PIE と衝突する。
#
#   liblibssh2_sys-....rlib(agent.o): relocation R_X86_64_32 against
#     .rodata.str1.1 can not be used when making a PIE object
#
# -fPIC を渡す道は三つ試して、どれも届かなかった。素の CFLAGS は bootstrap が
# 立てる CFLAGS_<triple> に負ける。その名前で渡しても tool を建てる経路では
# 効かない。config.toml の [target.*] の cc を包んでも同じだった。
#
# build.rs が LIBSSH2_SYS_USE_PKG_CONFIG という逃げ道を持っている。これは
# build script が環境から直接読むので、bootstrap が上書きする余地が無い。
# 建てないものは壊れない。
#
# 代償として、出来た cargo は package の libssh2 を要る。rustc の方は
# 自己完結のままなので、そちらは影響を受けない。
LIBSSH2_SYS_USE_PKG_CONFIG=1
export LIBSSH2_SYS_USE_PKG_CONFIG

TU=$(echo "$TRIPLE" | tr - _)
eval "CFLAGS_${TU}=-fPIC; export CFLAGS_${TU}"
eval "CXXFLAGS_${TU}=-fPIC; export CXXFLAGS_${TU}"
# bootstrap を経由しない build script のために素の方も置く。
CFLAGS="-fPIC"
CXXFLAGS="-fPIC"
export CFLAGS CXXFLAGS
LD_LIBRARY_PATH=$BOOT/lib:/usr/lib/gcc80
export LD_LIBRARY_PATH
python3 x.py dist rustc rust-std cargo

say "成果物を集める"
find build -name "*-${TRIPLE}.tar.xz" -print | sed 's/^/  /'
for part in rustc rust-std cargo; do
	f=$(find build -name "${part}-*-${TRIPLE}.tar.xz" | head -1)
	[ -n "$f" ] || { echo "  !!! ${part} の tarball が無い" >&2; exit 1; }
	cp "$f" "$OUT/"
done
ls -l "$OUT" | sed 's/^/  /'
