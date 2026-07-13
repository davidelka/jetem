#!/bin/sh
# demo.sh — a hands-on tour of jetem's escape-sequence support (M11–M15).
#
# Run it *inside* a jetem window:  sh examples/demo.sh
# Each test prints a label + what you should SEE, then the actual result line
# right below it — compare the two. \033 is ESC; the sequences are exactly what
# a shell/program would emit.

hr() { printf '\n\033[1;4m%s\033[0m\n' "$1"; }   # bold+underline section header

hr "1. Text attributes (SGR)"
printf '  should see: bold, faded, underlined, struck-through, reversed, italic\n'
printf '  actual:     '
printf '\033[1mBOLD\033[0m \033[2mDIM\033[0m \033[4mUNDER\033[0m \033[9mSTRIKE\033[0m \033[7mREV\033[0m \033[3mITAL\033[0m\n'
printf '  should see: the middle word invisible (concealed, SGR 8)\n'
printf '  actual:     visible \033[8mCONCEALED\033[0m back\n'
printf '  should see: dim-red text, then underlined+struck text\n'
printf '  actual:     \033[2;31mdim red\033[0m  \033[4;9munder+strike\033[0m\n'

hr "2. Absolute cursor column (CHA 'G')"
printf '  should see: aaaaXXaaaa   (jump to column 5, overwrite)\n'
printf '  actual:     '
printf 'aaaaaaaaaa\033[5GXX\n'

hr "3. In-line editing (DCH / ICH / ECH)"
printf '  should see: abef     (DCH: to col 3, delete 2)\n'
printf '  actual:     '
printf 'abcdef\033[3G\033[2P\n'
printf '  should see: ab  ef   (ECH: blank 2 at col 3, in place)\n'
printf '  actual:     '
printf 'abcdef\033[3G\033[2X\n'
printf '  should see: a   bcdef  (ICH: 3 blanks at col 2; nothing drops on a wide line)\n'
printf '  actual:     '
printf 'abcdef\033[2G\033[3@\n'

hr "4. Save / restore cursor (DECSC/DECRC + CSI s/u)"
printf '  should see: BBAA     (ESC 7 save, print AAAA, ESC 8 restore, print BB)\n'
printf '  actual:     '
printf '\0337AAAA\0338BB\n'
printf '  should see: ZZLLO    (CSI s save, HELLO, CSI u restore, ZZ)\n'
printf '  actual:     '
printf '\033[sHELLO\033[uZZ\n'

hr "5. Interactive (not scriptable — try these yourself)"
cat <<'EOF'
  - Line editing: type a long command, arrow/Ctrl-A into the middle, insert &
    delete chars — it should stay clean (this drives CHA + DCH + ICH via zsh).
  - Mouse (M14): run  htop  or  vim , then click / scroll / drag.
    Hold Shift while dragging to select text locally instead.
  - Bracketed paste (M14): copy a multi-line block elsewhere, then Ctrl-Shift-V
    here — it pastes as one block instead of running each line.
  - Rich output (M11): run a command with JSON/tabular output, then Ctrl-A t.
  - Theme (M13): Ctrl-A y cycles presets, Ctrl-A p flips the background.
EOF
printf '\n'
