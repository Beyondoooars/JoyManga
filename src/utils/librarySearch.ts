import type { Comic } from '../bindings.ts'

/** 多个词可分别命中标题、作者、标签或编号，全部命中时保留。 */
export function filterLibrary(comics: Comic[], query: string): Comic[] {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  if (terms.length === 0) return comics
  return comics.filter((comic) => {
    const text = [String(comic.id), comic.name, ...comic.author, ...comic.tags].join(' ').toLocaleLowerCase()
    return terms.every((term) => text.includes(term))
  })
}
