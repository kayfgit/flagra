#!/usr/bin/env bash
# Builds the small ffmpeg.exe flagra ships: only what flagra uses (screen capture
# with ddagrab, GPU scaling and download, x264, AAC, MPEG-TS and MP4), statically
# linked and cross-compiled for 64-bit Windows from Linux (Ubuntu: apt install
# mingw-w64 nasm pkg-config make).
#
# Writes to $OUT (default ./out): ffmpeg.exe, COPYING.GPLv2, README.txt, and the
# exact sources it was built from plus this script, which the GPL asks us to offer.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=version.env
source "$here/version.env"
tag="ffmpeg-$FFMPEG_VERSION-$BUILD"
out=$(mkdir -p "${OUT:-out}" && cd "${OUT:-out}" && pwd)
work=$(mkdir -p "${WORK:-build}" && cd "${WORK:-build}" && pwd)
prefix="$work/prefix"
host=x86_64-w64-mingw32
jobs=$(nproc)

cd "$work"
ffmpeg_src="ffmpeg-$FFMPEG_VERSION.tar.xz"
x264_src="x264-$X264_REF.tar.bz2"
[ -f "$ffmpeg_src" ] || curl -fsSL -o "$ffmpeg_src" "https://ffmpeg.org/releases/$ffmpeg_src"
[ -f "$x264_src" ] || curl -fsSL -o "$x264_src" "https://code.videolan.org/videolan/x264/-/archive/$X264_REF/$x264_src"
rm -rf ffmpeg x264 && mkdir ffmpeg x264
tar -xf "$ffmpeg_src" -C ffmpeg --strip-components=1
tar -xf "$x264_src" -C x264 --strip-components=1

# x264: 8-bit 4:2:0 only (flagra feeds it NV12), library only, no extras.
(
  cd x264
  ./configure --host="$host" --cross-prefix="$host-" --prefix="$prefix" \
    --enable-static --disable-cli --disable-opencl --disable-avs --disable-swscale \
    --disable-lavf --disable-ffms --disable-gpac --disable-lsmash \
    --bit-depth=8 --chroma-format=420
  make -j"$jobs"
  make install
)

# ffmpeg: everything off, then exactly what flagra's commands need.
#  recording: f32le audio from a pipe + ddagrab -> setpts -> fps -> scale_d3d11 ->
#             hwdownload -> format, encoded with libx264 and aac into MPEG-TS on a pipe
#  saving:    MPEG-TS from a file -> MP4, stream copy (h264/aac parsers, aac_adtstoasc)
#  aresample/aformat/scale/null/anull are what ffmpeg inserts on its own for conversions.
(
  cd ffmpeg
  PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig" ./configure \
    --prefix="$prefix" --target-os=mingw32 --arch=x86_64 \
    --cross-prefix="$host-" --enable-cross-compile \
    --pkg-config=pkg-config --pkg-config-flags=--static \
    --extra-cflags="-I$prefix/include" --extra-ldflags="-L$prefix/lib -static" \
    --disable-everything --disable-autodetect --disable-doc --disable-debug \
    --disable-network --disable-ffplay --disable-ffprobe --disable-avdevice \
    --enable-gpl --enable-libx264 --enable-w32threads --enable-d3d11va \
    --enable-filter=ddagrab,scale_d3d11,hwdownload,format,fps,setpts,scale,aresample,aformat,null,anull \
    --enable-encoder=libx264,aac \
    --enable-decoder=pcm_f32le,h264,aac \
    --enable-parser=h264,aac \
    --enable-demuxer=pcm_f32le,mpegts \
    --enable-muxer=mpegts,mp4 \
    --enable-bsf=aac_adtstoasc,h264_mp4toannexb \
    --enable-protocol=file,pipe
  make -j"$jobs"
  "$host-strip" -o "$out/ffmpeg.exe" ffmpeg.exe
  cp COPYING.GPLv2 "$out/COPYING.GPLv2"
)

cp "$work/$ffmpeg_src" "$work/$x264_src" "$here/build.sh" "$here/version.env" "$out/"
cat > "$out/README.txt" <<EOF
This folder holds ffmpeg $FFMPEG_VERSION with x264, built for flagra with only the
parts flagra uses (see build.sh). flagra runs it as a separate program.

ffmpeg and x264 are free software under the GNU General Public License, version 2
or later: see COPYING.GPLv2. The exact source code this ffmpeg.exe was built from,
and the script that built it, are published at
https://github.com/kayfgit/flagra/releases/tag/$tag
EOF
echo "built $tag: $(du -h "$out/ffmpeg.exe" | cut -f1)"
