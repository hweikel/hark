import type { Tile } from "./types";

/**
 * Search is deliberately dumb: prefix matching, no fuzziness, no typo
 * tolerance. Misspell something and you find nothing, which is the behaviour
 * asked for and the behaviour nvALT has.
 *
 * A tile survives if the query is a prefix of its artist, its title, or any
 * song title on it — so typing "s" keeps The Strokes (article stripped) and
 * also keeps an album merely containing a song called "Sabres".
 *
 * Matching is per-word as well as per-string, so "pepper" finds "Sgt. Pepper's
 * Lonely Hearts Club Band". Strict whole-string-only matching would mean the
 * only way to reach that album is to start typing at "sgt".
 *
 * Accents are the one thing it is *not* strict about — see fold().
 */

const SPLIT = /[\s\-_/(),.:;!?"'\[\]]+/;

/**
 * Fold a string to what you can actually type on a US keyboard.
 *
 * "Dumb" search is only useful if what you type can reach what you own, and a
 * good share of this collection isn't reachable without a compose key — Björk,
 * Eivør, Árstíðir, Angèle, Sinéad O'Connor. Typing "bjork" matching nothing
 * reads as a broken search, not as a strict one.
 *
 * Two passes, because they cover disjoint cases. NFD splits a letter from its
 * accent so the accent can be dropped (é → e), but does nothing for letters
 * that are their own codepoint rather than a base plus a mark — ø, ð and ß
 * survive NFD untouched, so they need the table.
 *
 * The combining range is spelled out as U+0300–U+036F rather than \p{M} on
 * purpose: Japanese dakuten is also a combining mark (U+3099), and a blanket
 * strip would quietly turn ライブ into ライフ. Restricting the range, then
 * recomposing with NFC, leaves every non-Latin script byte-identical to what
 * it was — Hangul and kana decompose and recompose losslessly.
 */
const LETTERS: Record<string, string> = {
  ø: "o",
  ð: "d",
  þ: "th",
  đ: "d",
  ł: "l",
  æ: "ae",
  œ: "oe",
  ß: "ss",
};

function fold(text: string): string {
  return text
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[øðþđłæœß]/g, (c) => LETTERS[c] ?? c)
    .normalize("NFC");
}

function strip(text: string): string {
  const normalized = fold(text.toLowerCase()).trim();
  return normalized.startsWith("the ") ? normalized.slice(4).trim() : normalized;
}

/** Every string a query can match this tile on. Built once per scan. */
export function tokensFor(tile: Tile): string[] {
  const tokens = new Set<string>();
  const sources = [tile.artist, tile.title, ...tile.tracks.map((t) => t.title)];

  for (const source of sources) {
    const stripped = strip(source);
    if (!stripped) continue;
    tokens.add(stripped);
    for (const word of stripped.split(SPLIT)) {
      if (word) tokens.add(word);
    }
  }

  return [...tokens];
}

export function normalizeQuery(query: string): string {
  return strip(query);
}

export function matches(tokens: string[], query: string): boolean {
  if (!query) return true;
  return tokens.some((token) => token.startsWith(query));
}
