#!/bin/sh
# Runs inside the `backup` compose service (and the restore drill).
#   eunomia-backup loop              nightly at 03:00 local time (TZ), forever
#   eunomia-backup now [prefix]      one encrypted export now (prefix: manual|daily|weekly)
#   eunomia-backup list              list backup files
#   eunomia-backup restore FILE [--wipe]   FILE is a name in /backups or a path
# Files are `surreal export` output, encrypted then authenticated (encrypt-then-MAC):
#   line 1: "EUNOMIA-BK2 <salt hex>"; then AES-256-CBC ciphertext; then a 64-char hex HMAC-SHA256 of
#   everything before it. Key and IV come from PBKDF2 of BACKUP_ENCRYPTION_KEY and the salt; the MAC
#   key is HMAC(key, "eunomia-backup-mac"). Restore checks the MAC before decrypting, so a tampered
#   file is refused. Older files (plain `openssl enc -aes-256-cbc -pbkdf2`, no header) still restore:
#   openssl enc -d -aes-256-cbc -pbkdf2 -pass env:BACKUP_ENCRYPTION_KEY -in X.surql.enc
# With org tenancy (a `control` database exists) one backup is a directory, NAME/, holding one
# file per database of the namespace (control.surql.enc, org_<uuid>.surql.enc, and the old
# single database until you remove it). Without it (a pre-tenancy install, the restore drill) a
# backup is the single file NAME.surql.enc as before.
# Uploaded documents' files are not in any export: when the documents folder (DOCUMENTS_DIR: the
# `file:` bucket folder, /data/documents in the database's data volume, mounted here as
# /surreal-data/documents by docker-compose.yml) exists and is not empty, a tenancy backup also holds
# documents.tar.enc (a tar of it, encrypted the same way), and restoring that backup unpacks it back.
# A bucket whose backend is S3 is not copied: back that bucket up with its own tools.
set -eu

ENDPOINT="${SURREAL_ENDPOINT:-http://surrealdb:8000}"
USER_="${SURREAL_USER:-root}"
PASS_="${SURREAL_PASS:-root}"
NS="${SURREAL_NS:-eunomia}"
DB="${SURREAL_DB:-eunomia}"
DIR="${BACKUP_DIR:-/backups}"
KEEP_DAILY="${KEEP_DAILY:-14}"
KEEP_WEEKLY="${KEEP_WEEKLY:-8}"
DOCS_DIR="${DOCUMENTS_DIR:-/documents}"

log() { echo "[backup $(date -u +%FT%TZ)] $*"; }
die() { log "ERROR: $*" >&2; exit 1; }

# The surreal CLI to use. The pre-upgrade backup (scripts/upgrade-surreal-v3.sh) points this at a copy
# of the RUNNING server's own binary, so a 2.x server is never exported with the 3.x CLI baked into this image.
SURREAL="${SURREAL_BIN:-surreal}"

KEY_FILE="${BACKUP_KEY_FILE:-$DIR/.backup-key}"
MAGIC="EUNOMIA-BK2"

# The key comes from BACKUP_ENCRYPTION_KEY (.env, recommended). If it is unset, use the key file in
# the backup volume, creating one on first start. That file sits next to the backups, so anyone who
# copies the volume gets both: set the key in .env to keep them apart.
need_key() {
  if [ -z "${BACKUP_ENCRYPTION_KEY:-}" ]; then
    if [ ! -s "$KEY_FILE" ]; then
      mkdir -p "$(dirname "$KEY_FILE")"
      ( umask 077; openssl rand -base64 32 > "$KEY_FILE" ) || die "could not create $KEY_FILE"
      log "============================================================"
      log "BACKUP_ENCRYPTION_KEY was not set, so a key was generated and saved to $KEY_FILE."
      log "COPY IT SOMEWHERE SAFE NOW (a password manager). Without it your backups cannot be read."
      log "Show it:  docker compose exec backup cat $KEY_FILE"
      log "Better: put BACKUP_ENCRYPTION_KEY=<that value> in .env so the key is not stored beside the backups."
      log "============================================================"
    fi
    BACKUP_ENCRYPTION_KEY="$(cat "$KEY_FILE")"; export BACKUP_ENCRYPTION_KEY
  fi
}

# derive SALT_HEX: sets ENC_KEY, ENC_IV, MAC_KEY (hex)
derive() {
  kv="$(openssl enc -aes-256-cbc -pbkdf2 -iter 200000 -S "$1" -pass env:BACKUP_ENCRYPTION_KEY -P < /dev/null)" || return 1
  ENC_KEY="$(echo "$kv" | sed -n 's/^key *= *//p')"; ENC_IV="$(echo "$kv" | sed -n 's/^iv *= *//p')"
  MAC_KEY="$(printf eunomia-backup-mac | openssl dgst -sha256 -mac HMAC -macopt "hexkey:$ENC_KEY" -r | cut -d' ' -f1)"
  [ -n "$ENC_KEY" ] && [ -n "$ENC_IV" ] && [ -n "$MAC_KEY" ]
}
mac_of() { openssl dgst -sha256 -mac HMAC -macopt "hexkey:$MAC_KEY" -r | cut -d' ' -f1; } # stdin -> hex


export_db() { # export one database to stdout
  "$SURREAL" export --log none --endpoint "$ENDPOINT" --user "$USER_" --pass "$PASS_" --ns "$NS" --db "$1" -
}

# The databases to back up: control, every org_<uuid> and $DB (the old single database) when the
# tenancy layout is there; just $DB otherwise.
list_dbs() {
  # A failed command must stop the backup: falling back to $DB here would back up only the stale
  # pre-move database and still log success. Only "the namespace answered but has no control
  # database" (pre-tenancy, or a 2.x server) means the single-database layout.
  info="$(echo 'INFO FOR NS;' | "$SURREAL" sql --endpoint "$ENDPOINT" --user "$USER_" --pass "$PASS_" --ns "$NS" --hide-welcome --json 2>/dev/null)" \
    || die "could not list the databases of namespace $NS (is SurrealDB reachable at $ENDPOINT and are the credentials right?)"
  echo "$info" | grep -q '"databases"' || die "unexpected answer to INFO FOR NS: $(echo "$info" | head -c 300)"
  if ! echo "$info" | grep -q '"control"'; then echo "$DB"; return; fi
  { echo "$info" | grep -oE '"(control|org_[0-9a-f]{32})"[[:space:]]*:' | sed -E 's/^"([^"]+)".*/\1/'
    echo "$info" | grep -q "\"$DB\"[[:space:]]*:" && echo "$DB"; } | sort -u
}

encrypt_to() { # stdin -> $1, atomically; refuses an empty export
  tmp="$1.part"
  salt="$(openssl rand -hex 8)"; derive "$salt" || return 1
  { echo "$MAGIC $salt"; openssl enc -aes-256-cbc -K "$ENC_KEY" -iv "$ENC_IV"; } > "$tmp" || { rm -f "$tmp"; return 1; }
  # append the MAC (64 hex chars, no newline) of everything written so far
  m="$(mac_of < "$tmp")" || { rm -f "$tmp"; return 1; }
  printf '%s' "$m" >> "$tmp"
  [ "$(wc -c < "$tmp")" -gt 160 ] || { rm -f "$tmp"; return 1; }
  mv "$tmp" "$1"
}

sql_db() { # sql_db DB: statements on stdin, JSON on stdout
  "$SURREAL" sql --endpoint "$ENDPOINT" --user "$USER_" --pass "$PASS_" --ns "$NS" --db "$1" --hide-welcome --json 2>/dev/null
}

# row_counts DB: prints "<table> <rows>" for every table of DB, asked of the live server (before and
# after the export, see dump_to). `INFO FOR DB` lists the tables (names must be plain identifiers, so
# they are safe to put in a query); one `SELECT count() ... GROUP ALL` per table runs in a single call.
# Uses only the `count` function family, which the hardened server allows.
row_counts() {
  info="$(echo 'INFO FOR DB;' | sql_db "$1")" || return 1
  echo "$info" | grep -q '"tables"' || return 1
  tables="$(echo "$info" | sed -n 's/.*"tables":{\(.*\)}.*/\1/p' | grep -oE '"[^"]+":"DEFINE TABLE' | sed -E 's/^"([^"]+)".*/\1/')"
  [ -n "$tables" ] || return 0
  q=""
  for t in $tables; do
    case "$t" in *[!A-Za-z0-9_]*) log "table name $t in $1 is not a plain identifier, cannot verify it" >&2; return 1 ;; esac
    q="$q SELECT count() FROM \`$t\` GROUP ALL;"
  done
  res="$(echo "$q" | sql_db "$1")" || return 1
  echo "$res" | grep -q '"error"' && return 1
  # [[{"count":2}],[],[{"count":0}]] -> one line per statement, in table order
  counts="$(echo "$res" | sed -e 's/^\[\[*//' -e 's/\]*\]$//' -e 's/\],\[/\n/g' | sed -E 's/.*"count":([0-9]+).*/\1/; s/^[^0-9].*$/0/; s/^$/0/')"
  [ "$(echo "$counts" | wc -l)" -eq "$(echo "$tables" | wc -l)" ] || return 1
  echo "$tables" | { while read -r t; do read -r c <&3; echo "$t $c"; done; } 3<<EOF
$counts
EOF
}

# dump_counts DUMP: prints "<table> <rows>" for every "-- TABLE DATA:" section of a `surreal export`:
# the records at the top level of its `INSERT [ ... ];` / `INSERT RELATION [ ... ];` statements. One
# global regex walks the file: whole comment lines (`--` at a line start, so quotes in them are inert),
# whole quoted strings and ids ('..', "..", `..`, ⟨..⟩: skipped, so braces inside values never count,
# and they may span lines: 2.x exports keep raw newlines inside strings), and the brackets in between.
# A nested object or array is deeper than 1. perl is in every Debian image (perl-base is essential).
dump_counts() {
  perl -0777 -ne '
    my (%c, $t, $d);
    while (/^(--[^\n]*)|(\x27[^\x27\\]*(?:\\.[^\x27\\]*)*\x27|"[^"\\]*(?:\\.[^"\\]*)*"|`[^`\\]*(?:\\.[^`\\]*)*`|\xe2\x9f\xa8(?:[^\\\xe2]|\\.|\xe2(?!\x9f\xa9))*\xe2\x9f\xa9)|([\[\]{}])/gm) {
      if (defined $1) { if ($1 =~ /^-- TABLE DATA: (.*)$/) { $t = $1; $c{$t} += 0; $d = 0 } next }
      next if defined $2 or not defined $t;
      if ($3 eq "{") { $c{$t}++ if $d == 1; $d++ } elsif ($3 eq "[") { $d++ } else { $d-- }
    }
    END { print "$_ $c{$_}\n" for sort keys %c }' "$1"
}

# verify_dump DUMP BEFORE AFTER DB: every table's rows in the dump must equal the server's count. The
# server keeps serving while it is exported, so its counts are taken just before and just after the
# export: a table that did not change in between must match exactly, one written meanwhile must lie
# between its two counts. An export cut short by a dying server, or missing rows for any other reason,
# holds fewer (and such an export exits 0 more often than you'd think).
verify_dump() {
  got="$(dump_counts "$1")" || { log "could not read the export of $4" >&2; return 1; }
  bad="$( { echo "$2" | sed 's/^/B /'; echo "$3" | sed 's/^/A /'; echo "$got" | sed 's/^/E /'; } | awk '
    NF == 3 && $1 == "B" { b[$2] = $3; seen[$2] = 1 }
    NF == 3 && $1 == "A" { a[$2] = $3; seen[$2] = 1 }
    NF == 3 && $1 == "E" { e[$2] = $3 }
    END {
      for (t in seen) {
        lo = (t in b) ? b[t] : a[t]; hi = (t in a) ? a[t] : b[t]
        if (lo + 0 > hi + 0) { x = lo; lo = hi; hi = x }
        n = e[t] + 0
        if (n < lo + 0 || n > hi + 0) printf " %s(%s rows counted, %d in the export)", t, (lo == hi ? lo : lo ".." hi), n
      }
    }')"
  [ -z "$bad" ] || { log "export of $4 does not hold the rows the server counted:$bad" >&2; return 1; }
}

# dump_to DB OUT: count rows, export, count again, check the dump holds exactly the counted rows, then
# encrypt. sh has no pipefail, so the export goes to a private temp file (shredded after) whose exit
# status, last line and per-table rows are checked, and the server must still answer: a dump that died
# midway never becomes a backup. When the counts changed during the export (writes, not a short dump)
# and the dump falls outside them, it is exported again, at most 3 times in all.
dump_to() {
  attempt=1
  while :; do
    before="$(row_counts "$1")" || { log "could not count the rows of $1 before exporting" >&2; return 1; }
    x="$(mktemp)" || return 1
    rc=1; again=""
    if export_db "$1" > "$x" && tail -c 4096 "$x" | grep -v '^--' | grep . | tail -n 1 | grep -q ';$' \
       && "$SURREAL" isready --endpoint "ws://${ENDPOINT#*://}" >/dev/null 2>&1; then
      if ! after="$(row_counts "$1")"; then
        log "could not count the rows of $1 after exporting" >&2
      elif verify_dump "$x" "$before" "$after" "$1"; then
        encrypt_to "$2" < "$x"; rc=$?
      elif [ "$before" != "$after" ] && [ "$attempt" -lt 3 ]; then
        again=1
      fi
    fi
    shred -u -f "$x" 2>/dev/null || rm -f "$x"
    [ -n "$again" ] || return "$rc"
    attempt=$((attempt + 1))
    log "the rows of $1 changed while it was exported; exporting it again (attempt $attempt of 3)" >&2
  done
}

do_backup() {
  prefix="${1:-manual}"
  need_key
  mkdir -p "$DIR"
  name="$prefix-$(date -u +%Y%m%dT%H%M%SZ)"
  dbs="$(list_dbs)" || exit 1
  # --log none keeps log lines out of stdout (they would corrupt the dump).
  if [ "$dbs" = "$DB" ]; then
    out="$DIR/$name.surql.enc"
    dump_to "$DB" "$out" || { rm -f "$out"; die "export failed (is SurrealDB reachable at $ENDPOINT and are the credentials right?)"; }
  else
    out="$DIR/$name"
    mkdir -p "$out.part"
    for d in $dbs; do
      dump_to "$d" "$out.part/$d.surql.enc" || { rm -rf "$out.part"; die "export of database $d failed (is SurrealDB reachable at $ENDPOINT and are the credentials right?)"; }
    done
    if [ -d "$DOCS_DIR" ] && [ -n "$(ls -A "$DOCS_DIR" 2>/dev/null)" ]; then
      tar -C "$DOCS_DIR" -cf - . | encrypt_to "$out.part/documents.tar.enc" \
        || { rm -rf "$out.part"; die "could not archive the documents in $DOCS_DIR"; }
    fi
    mv "$out.part" "$out"
  fi
  log "wrote $out ($(du -sk "$out" | cut -f1) KB, databases: $(echo $dbs))"
  rotate daily "$KEEP_DAILY"
  rotate weekly "$KEEP_WEEKLY"
}

rotate() { # keep the newest $2 backups (files or directories) named $1-*
  ls -1d "$DIR"/"$1"-* 2>/dev/null | grep -v '\.part$' | sort -r | tail -n +"$(($2 + 1))" | while read -r f; do
    rm -rf "$f"; log "rotated out $f"
  done
}

decrypt_to() { # decrypt_to FILE OUT
  first="$(head -n1 "$1" | tr -d '\000')"
  case "$first" in
    "$MAGIC "*)
      derive "${first#"$MAGIC "}" || die "could not derive keys for $1"
      body=$(( $(wc -c < "$1") - 64 )); hl=$(( ${#first} + 1 ))
      [ "$body" -gt "$hl" ] || die "$1 is truncated"
      [ "$(head -c "$body" "$1" | mac_of)" = "$(tail -c 64 "$1")" ] \
        || die "$1 failed its integrity check: wrong BACKUP_ENCRYPTION_KEY, or the file was modified or corrupted"
      head -c "$body" "$1" | tail -c +"$((hl + 1))" | openssl enc -d -aes-256-cbc -K "$ENC_KEY" -iv "$ENC_IV" > "$2" 2>/dev/null \
        || die "could not decrypt $1" ;;
    *) # legacy format: no header, no MAC
      openssl enc -d -aes-256-cbc -pbkdf2 -pass env:BACKUP_ENCRYPTION_KEY -in "$1" > "$2" 2>/dev/null \
        || die "could not decrypt $1: wrong BACKUP_ENCRYPTION_KEY or corrupt file" ;;
  esac
}

restore_one() { # restore_one FILE DB [--wipe]
  plain="$(mktemp)"; trap 'rm -f "$plain"' EXIT
  decrypt_to "$1" "$plain"
  if [ "${3:-}" = "--wipe" ]; then
    log "wiping database $NS/$2"
    echo "REMOVE DATABASE IF EXISTS \`$2\`;" | "$SURREAL" sql --endpoint "$ENDPOINT" --user "$USER_" --pass "$PASS_" --ns "$NS" --hide-welcome >/dev/null \
      || die "wipe failed"
  fi
  echo "DEFINE DATABASE IF NOT EXISTS \`$2\`;" | "$SURREAL" sql --endpoint "$ENDPOINT" --user "$USER_" --pass "$PASS_" --ns "$NS" --hide-welcome >/dev/null \
    || die "could not create database $2"
  "$SURREAL" import --endpoint "$ENDPOINT" --user "$USER_" --pass "$PASS_" --ns "$NS" --db "$2" "$plain" \
    || die "import of $2 failed. If the database already has data, retry with --wipe (this deletes it first). A big database can hit the server's 60 s transaction limit: restore with backend/scripts/restore.sh, which runs the import on a server without it."
  rm -f "$plain"
}

do_restore() {
  need_key
  f="${1:-}"; [ -n "$f" ] || die "usage: restore NAME [--wipe]"
  [ -e "$f" ] || f="$DIR/$f"
  [ -e "$f" ] || die "no such backup: $1 (try: list)"
  if [ -d "$f" ]; then
    for file in "$f"/*.surql.enc; do
      d="$(basename "$file" .surql.enc)"
      restore_one "$file" "$d" "${2:-}"
    done
    if [ -f "$f/documents.tar.enc" ]; then
      mkdir -p "$DOCS_DIR" 2>/dev/null; [ -w "$DOCS_DIR" ] || die "$f holds documents but $DOCS_DIR is not writable here"
      plain="$(mktemp)"
      decrypt_to "$f/documents.tar.enc" "$plain"
      [ "${2:-}" != "--wipe" ] || find "$DOCS_DIR" -mindepth 1 -delete
      tar -C "$DOCS_DIR" -xf "$plain" || { rm -f "$plain"; die "could not unpack the documents into $DOCS_DIR"; }
      rm -f "$plain"
      log "restored the documents into $DOCS_DIR"
    fi
  else
    restore_one "$f" "$DB" "${2:-}"
  fi
  log "restored $f"
}

secs_to_next_3am() {
  now="$(date +%s)"; t="$(date -d 03:00 +%s)"
  [ "$t" -gt "$now" ] || t="$(date -d 'tomorrow 03:00' +%s)"
  echo $((t - now))
}

case "${1:-}" in
  loop)
    need_key
    log "nightly backups to $DIR at 03:00 (TZ=${TZ:-UTC}), keeping $KEEP_DAILY daily + $KEEP_WEEKLY weekly"
    while :; do
      sleep "$(secs_to_next_3am)"
      ( do_backup daily ) || log "nightly backup failed, will retry tomorrow"
      [ "$(date +%u)" = 7 ] && { ls -1dt "$DIR"/daily-* | head -1 | while read -r f; do
        cp -R "$f" "$DIR/weekly-${f##*/daily-}"; done; rotate weekly "$KEEP_WEEKLY"; }
    done ;;
  now) do_backup "${2:-manual}" ;;
  list) ls -lh "$DIR" ;;
  restore) shift; do_restore "$@" ;;
  *) die "usage: eunomia-backup loop | now [prefix] | list | restore NAME [--wipe]" ;;
esac
