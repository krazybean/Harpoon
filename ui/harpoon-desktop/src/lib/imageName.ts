/** An image as the journal records it: `repo[:tag]`, optionally followed by ` (shortid)`. Split so a long tag can be shortened without losing any of it. */
export type ImageParts = { repo: string; tag: string | null; tagShown: string | null; id: string | null };

const TAG_MAX = 28;

/** Keep the start and the end of a long text; the middle is replaced by "…". The full text is always kept elsewhere. */
export function middleShort(text: string, max = TAG_MAX): string {
  if (text.length <= max) return text;
  const keep = max - 1, head = Math.ceil(keep / 2);
  return `${text.slice(0, head)}…${text.slice(text.length - (keep - head))}`;
}

export function imageParts(value: string): ImageParts {
  const m = /^(.*?)(?: \(([^()]+)\))?$/.exec(value) ?? [];
  const ref = m[1] ?? value;
  // a ':' after the last '/' starts the tag (a ':' before it is a registry port)
  const colon = ref.lastIndexOf(":");
  const hasTag = colon > ref.lastIndexOf("/");
  const tag = hasTag ? ref.slice(colon + 1) : null;
  return { repo: hasTag ? ref.slice(0, colon) : ref, tag, tagShown: tag === null ? null : middleShort(tag), id: m[2] ?? null };
}
