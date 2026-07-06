#!/usr/bin/env bash
# rss-to-notes — a minimal Curator producer: RSS/Atom feed -> markdown
# notes in a vault directory.
#
#   rss-to-notes.sh <feed-url> <out-dir>
#   rss-to-notes.sh https://example.com/feed.xml ~/vault/clips
#
# The whole integration story in one script: any producer that writes
# conforming markdown+frontmatter into the vault is a valid producer
# (contracts/kp-note/v1.md). This one writes plain notes WITHOUT a
# kp_id, so the plane assigns each the implicit `path:` identity — the
# filename is derived from the item link, so re-running the script on
# the same feed is idempotent (existing notes are left untouched, which
# also preserves any enrichment the plane added since).
#
# Dependencies: curl + awk + shasum/sha256sum. No feed library — RSS 2.0
# and Atom are handled by a small awk state machine that is deliberately
# conservative: items missing a link or title are skipped, HTML is
# stripped crudely, and entities are unescaped minimally. For a gnarly
# feed, write a real producer; this is the 150-line sketch of one.

set -euo pipefail

usage() {
    echo "usage: $0 <feed-url> <out-dir>" >&2
    exit 2
}

[ $# -eq 2 ] || usage
feed_url=$1
out_dir=$2

command -v curl >/dev/null || { echo "rss-to-notes: curl not found" >&2; exit 1; }
if command -v sha256sum >/dev/null; then
    sha() { sha256sum | awk '{print $1}'; }
else
    sha() { shasum -a 256 | awk '{print $1}'; }
fi

mkdir -p "$out_dir"

feed_xml=$(curl --fail --silent --show-error --location --max-time 60 "$feed_url")

# One record per item/entry: title \t link \t date \t description.
# Handles RSS 2.0 (<item><title><link><pubDate><description>) and Atom
# (<entry><title><link href=...><updated><summary>). The awk program
# splits the document on "<" — each awk record is then one tag (up to
# the first ">") plus its trailing text — and ACCUMULATES text between
# an opening and a closing title/description tag, so CDATA sections and
# HTML markup inside descriptions survive (as text; the markup itself
# is dropped).
records=$(printf '%s' "$feed_xml" | awk '
    function unescape(s) {
        gsub(/&lt;/, "<", s); gsub(/&gt;/, ">", s)
        gsub(/&quot;/, "\"", s); gsub(/&#39;/, "\x27", s)
        gsub(/&amp;/, "\\&", s); return s
    }
    function clean(s) {
        gsub(/\]\]>?/, "", s)             # CDATA terminators
        gsub(/[ \t]+/, " ", s)
        sub(/^ /, "", s); sub(/ $/, "", s)
        return unescape(s)
    }
    function emit() {
        if (length(desc) > 600) desc = substr(desc, 1, 600) "..."
        if (title != "" && link != "")
            printf "%s\t%s\t%s\t%s\n", title, link, date, desc
        title = ""; link = ""; date = ""; desc = ""
    }
    BEGIN { RS = "<"; in_item = 0; grab = "" }
    {
        rec = $0
        gsub(/[\r\n]/, " ", rec)
        tag = rec; sub(/>.*/, "", tag)    # the tag part (up to ">")
        text = ""
        if (rec ~ />/) { text = rec; sub(/^[^>]*>/, "", text) }
        if (rec ~ /^!\[CDATA\[/) {        # CDATA start: content is text
            text = rec; sub(/^!\[CDATA\[/, "", text)
        }
    }
    tag ~ /^(item|entry)([ \t]|$)/  { in_item = 1; next }
    tag ~ /^\/(item|entry)$/        { emit(); in_item = 0; grab = ""; next }
    !in_item                        { next }
    tag == "/title"                 { grab = ""; next }
    tag ~ /^\/(description|summary)$/ { grab = ""; next }
    tag ~ /^title([ \t]|$)/ && title == "" { grab = "title"; title = clean(text); next }
    tag ~ /^(description|summary)([ \t]|$)/ && desc == "" { grab = "desc"; desc = clean(text); next }
    tag ~ /^link([ \t]|$)/ && link == "" {
        if (tag ~ /href="/) {             # Atom: <link href="..."/>
            l = tag; sub(/.*href="/, "", l); sub(/".*/, "", l); link = l
        } else {                          # RSS: <link>url</link>
            link = clean(text)
        }
        next
    }
    tag ~ /^(pubDate|updated|published)([ \t]|$)/ && date == "" { date = clean(text); next }
    # Inside an accumulating element: markup records contribute their
    # trailing text (the markup itself is dropped).
    grab == "title" { title = clean(title " " text) }
    grab == "desc"  { desc  = clean(desc " " text) }
    END { emit() }
')

if [ -z "$records" ]; then
    echo "rss-to-notes: no items found in $feed_url" >&2
    exit 1
fi

now_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)
created=0
skipped=0

while IFS=$'\t' read -r title link date desc; do
    # Deterministic filename from the item link: idempotent re-runs.
    slug=$(printf '%s' "$title" \
        | tr '[:upper:]' '[:lower:]' \
        | sed -e 's/[^a-z0-9]\{1,\}/-/g' -e 's/^-//' -e 's/-$//' \
        | cut -c1-60)
    hash=$(printf '%s' "$link" | sha | cut -c1-8)
    note_path="$out_dir/${slug:-untitled}-$hash.md"

    if [ -e "$note_path" ]; then
        skipped=$((skipped + 1))
        continue
    fi

    # kp-note/v1-conforming frontmatter, no kp_id: the plane assigns
    # `path:` identity. `source` carries the item URL; `created` is the
    # feed's own timestamp when present, else now.
    {
        echo '---'
        echo "title: \"$(printf '%s' "$title" | sed 's/"/\\"/g')\""
        echo "created: \"${date:-$now_utc}\""
        echo "source: \"$link\""
        echo 'tags: [clip, rss]'
        echo '---'
        echo
        echo "# $title"
        echo
        if [ -n "$desc" ]; then
            printf '%s\n\n' "$desc"
        fi
        echo "> Clipped from <$link> on $now_utc."
    } > "$note_path"
    echo "created $note_path"
    created=$((created + 1))
done <<< "$records"

echo "rss-to-notes: $created created, $skipped already present"
echo "next: curator ingest   # pick the new notes up into the index"
