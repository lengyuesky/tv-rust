type Item = Record<string, any>;
const compareText = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

export function compareSearchItems(a: Item, b: Item): number {
  return (
    (a.search_rank ?? Number.MAX_SAFE_INTEGER) -
      (b.search_rank ?? Number.MAX_SAFE_INTEGER) ||
    compareText(a.search_title || "", b.search_title || "") ||
    compareText(a.year || "", b.year || "") ||
    compareText(a.source_name || "", b.source_name || "") ||
    compareText(a.id, b.id)
  );
}

export function mergeSearchItems(previous: Item[], incoming: Item[]): Item[] {
  const films = new Map<string, Item>();
  for (const film of [...previous, ...incoming]) films.set(film.id, film);
  return [...films.values()].sort(compareSearchItems);
}
