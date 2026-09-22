#!/bin/sh
# cgd (NetBSD) と softraid crypto (OpenBSD) の性質を実機で確かめる。
#
# どちらも実装の前に知りたいことがある。
#
# cgd は keyslot を持たない。鍵は params ファイルの側にあるので、素直に
# storedkey を使うと消去が「普通のファイルを消す」ことになり、SD や SSD で
# 成立しない。shell_cmd（コマンドの stdout から鍵を読む）が道になるはずだが、
# 鍵をどう渡せばよいのかが man から読み取れない。
#
# softraid は鍵をファイルから渡せるのかを知りたい。パスフレーズを人が打つ
# しか無いなら、無人で導入できない。
#
# 使い捨て。答えが出たら消す。
set -e
say() { echo; echo "### $*"; }

case $(uname) in
NetBSD) ;;
OpenBSD) ;;
*) echo "この OS は見ない"; exit 0 ;;
esac

W=$HOME/crypt-probe
rm -rf "$W"; mkdir -p "$W"; cd "$W"
dd if=/dev/zero of="$W/disk.img" bs=1m count=64 2>/dev/null

if [ "$(uname)" = NetBSD ]; then
	say "版と道具"
	uname -r
	ls -l /sbin/cgdconfig

	say "cgdconfig の使い方"
	cgdconfig 2>&1 | head -25 || true

	say "storedkey の params を生成してみる"
	cgdconfig -g -o "$W/p-stored" -V none aes-xts 256 < /dev/null 2>&1 | head -5 || true
	if [ -f "$W/p-stored" ]; then
		echo "  --- 生成された params ---"
		sed 's/^/  /' "$W/p-stored"
	else
		echo "  生成できなかった"
	fi

	say "vnd を作る"
	vnconfig vnd0 "$W/disk.img" 2>&1 | sed 's/^/  /' || true
	ls -l /dev/vnd0d 2>/dev/null | sed 's/^/  /' || echo "  /dev/vnd0d が無い"

	say "shell_cmd を試す（鍵は生の 32 byte）"
	printf '#!/bin/sh\ndd if=/dev/zero bs=32 count=1 2>/dev/null\n' > "$W/keycmd"
	chmod +x "$W/keycmd"
	cat > "$W/p-shell" <<CONF
algorithm aes-xts;
iv-method encblkno1;
keylength 256;
verify_method none;
keygen shell_cmd "$W/keycmd";
CONF
	sed 's/^/  /' "$W/p-shell"
	echo "  --- cgdconfig cgd0 /dev/vnd0d p-shell ---"
	cgdconfig cgd0 /dev/vnd0d "$W/p-shell" 2>&1 | sed 's/^/  /' && {
		echo "  通った"
		ls -l /dev/cgd0d 2>/dev/null | sed 's/^/  /'
		cgdconfig -u cgd0 2>&1 | sed 's/^/  /' || true
	} || echo "  駄目だった"

	say "片付け"
	cgdconfig -u cgd0 2>/dev/null || true
	vnconfig -u vnd0 2>/dev/null || true
fi

if [ "$(uname)" = OpenBSD ]; then
	say "版と道具"
	uname -r
	ls -l /sbin/bioctl

	say "bioctl の使い方（鍵をファイルから渡せるか）"
	bioctl 2>&1 | head -25 || true

	say "man から鍵の渡し方"
	man bioctl 2>/dev/null | grep -B2 -A6 -iE 'passphrase|keydisk|key disk' | head -40 \
		|| echo "  man が読めない"

	say "softraid の状態"
	bioctl softraid0 2>&1 | head -10 || true

	say "vnd を作る"
	vnconfig vnd0 "$W/disk.img" 2>&1 | sed 's/^/  /' || true
	disklabel vnd0 2>&1 | tail -6 | sed 's/^/  /' || true

	say "片付け"
	vnconfig -u vnd0 2>/dev/null || true
fi

cd "$HOME"; rm -rf "$W"
echo; echo "### 済"
