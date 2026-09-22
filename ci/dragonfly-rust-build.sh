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
# libssh2 はここに書かない。既に入っている（rust と git と curl の依存）。
# 名前を挙げると pkg が upgrade を試み、古い ABI に依存する物を巻き添えで
# 消す。実際にそうなった:
#
#   Installed packages to be REMOVED:
#       curl: 8.10.0
#       git: 2.49.0
#       rust: 1.85.1        <- 種の rustc
#
# repo が移行中で、新しい libssh2 に合わせて建て直した rust がまだ無い。
pkg install -y "$LLVMPKG" zstd

# 入った物が在ることをここで確かめる。無いまま建て始めると、気づくのは
# 20 分以上あとの link 段階になる。
for lib in libzstd.a libzstd.so libssh2.so; do
	[ -e "/usr/local/lib/$lib" ] || { echo "/usr/local/lib/$lib が無い" >&2; exit 1; }
done
# libssh2 は pkg-config 経由で使うので、その定義が在ることも見る。
[ -e /usr/local/libdata/pkgconfig/libssh2.pc ] \
	|| { echo "libssh2.pc が無い" >&2; exit 1; }
# pkg install は依存の都合で既に入っている物を消すことがある。種を消された
# まま 20 分建ててから気づくのは高いので、ここで見る。二段目からは種を
# artifact から入れるので、pkg の rust が消えていても構わない。
if [ -z "$BOOTSTRAP_VER" ] && [ ! -x /usr/local/bin/rustc ]; then
	echo "pkg install が種の rustc を消した" >&2
	exit 1
fi
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
ROOT_DIR=$(pwd)
OUT=$ROOT_DIR/dist
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
# になる。
#
# **-Lnative ではなく -Clink-arg で渡すこと。** -Lnative は rustc が静的
# ライブラリを探す所にも効くので、build script が cargo:rustc-link-search で
# 指す OUT_DIR より先に /usr/local/lib が見られる。その結果、cc-rs が -fPIC で
# 建てた libssh2.a ではなく、pkg の非 PIC な /usr/local/lib/libssh2.a が rlib に
# 取り込まれ、cargo の link が PIE で落ちる。-Clink-arg なら最終 link にしか
# 効かず、どちらを取り込むかには影響しない。
RUSTFLAGS="-Clink-arg=-L/usr/local/lib"
export RUSTFLAGS
# bootstrap は自分の段ごとに RUSTFLAGS を組み直すので、そちらにも渡す。
RUSTFLAGS_BOOTSTRAP=$RUSTFLAGS
RUSTFLAGS_NOT_BOOTSTRAP=$RUSTFLAGS
export RUSTFLAGS_BOOTSTRAP RUSTFLAGS_NOT_BOOTSTRAP
# cc が link するときにも効かせる。
LIBRARY_PATH=/usr/local/lib
export LIBRARY_PATH
# SARACHI_PIC_PROBE を立てると cc がどう呼ばれたかを捕まえる。普段は不要。
if [ -n "${SARACHI_PIC_PROBE:-}" ]; then
	echo "### [下調べ] libssh2 の C を建てる。落ちるのを承知で cc の呼ばれ方を見る"
	CC_ENABLE_DEBUG_OUTPUT=1
	export CC_ENABLE_DEBUG_OUTPUT
fi

LD_LIBRARY_PATH=$BOOT/lib:/usr/lib/gcc80
export LD_LIBRARY_PATH
if [ -n "${SARACHI_PIC_PROBE:-}" ]; then
	# rustc 本体は建てない。落ちたのは stage1-tools の cargo なので、種の
	# rustc で cargo だけ建てれば同じ経路を通る。全部建てると 45 分かかる。
	#
	# **必ず exit 0 で終わること。** run が失敗すると vmactions は作業結果を
	# 持ち帰らない。一度それで 45 分走らせて手ぶらになった。
	set +e
	# -v 一つでは cargo の -vv にならず、build script の出力が出ない。
	# cc がどう呼ばれたかを見たいので二つ重ねる。
	python3 x.py build --stage 0 cargo -vv > "$WRK/x.log" 2>&1
	RC=$?
	set -e
	echo "  x.py の終了状態: $RC"

	mkdir -p "$ROOT_DIR/probe-out"
	# log は丸ごと持ち帰る。部分的に grep して当たらなければ何も見えない、
	# という形で既に二度外している。
	# 末尾だけ切ると、C を建てた所が落ちる。実際それで一度外した。
	# 要る所だけ抜いて持ち帰る。
	{
		grep -a 'libssh2' "$WRK/x.log"
		grep -a 'running:' "$WRK/x.log"
	} > "$ROOT_DIR/probe-out/libssh2.log" 2>/dev/null || true
	wc -l < "$ROOT_DIR/probe-out/libssh2.log" \
		| awk '{print "  libssh2 に触れる行 "$1" 件"}'

	# cc の呼ばれ方と、bootstrap が立てた環境を残す。ここが本題。
	{
		echo "=== agent.c の command line ==="
		grep -a 'agent\.c' "$WRK/x.log" | head -3
		echo
		echo "=== libssh2-sys の build script が見た環境 ==="
		grep -a -E 'CC_x86_64|CFLAGS_x86_64|CRATE_CC_NO_DEFAULTS' "$WRK/x.log" | head -10
		echo
		echo "=== PIE の文句 ==="
		grep -a -m3 -B2 -A2 'can not be used when making a PIE' "$WRK/x.log"
	} > "$ROOT_DIR/probe-out/pic.txt" 2>&1
	sed 's/^/  /' "$ROOT_DIR/probe-out/pic.txt" | cut -c1-200 | head -20

	# 問題の rlib そのものを持ち帰る。loose な .o を拾うと、別の build
	# ディレクトリの綺麗な方を掴む。実際それで「再現しない」と読み違えた。
	for R in $(find "$WRK" -name 'liblibssh2_sys-*.rlib' 2>/dev/null); do
		cp "$R" "$ROOT_DIR/probe-out/$(basename "$R")"
		echo "  $(basename "$R") を持ち帰る（$(wc -c < "$R") bytes）"
	done

	# 下調べは「測れたか」で判定する。建ったかどうかではない。
	exit 0
fi

python3 x.py dist rustc rust-std cargo

say "成果物を集める"
find build -name "*-${TRIPLE}.tar.xz" -print | sed 's/^/  /'
for part in rustc rust-std cargo; do
	f=$(find build -name "${part}-*-${TRIPLE}.tar.xz" | head -1)
	[ -n "$f" ] || { echo "  !!! ${part} の tarball が無い" >&2; exit 1; }
	cp "$f" "$OUT/"
done
ls -l "$OUT" | sed 's/^/  /'
