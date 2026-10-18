import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createPinia } from 'pinia'
import { useStore } from '../src/store.ts'

test('local archive metadata cannot overwrite the current account favorite state', () => {
  const store = useStore(createPinia())
  store.applySession({ sessionEpoch: 1, profile: { uid: 'first' } })
  store.recordFavorite(111, true, 1)
  store.showComic({ id: 111, name: 'local archive', is_favorite: false, chapterInfos: [] })
  assert.equal(store.favoriteStatuses.get(111), true)
  assert.equal(store.pickedComicFavoriteEpoch, undefined)
})

test('switching account clears favorite state and rejects late old-account updates', () => {
  const store = useStore(createPinia())
  store.applySession({ sessionEpoch: 1, profile: { uid: 'first' } })
  store.recordFavorite(111, true, 1)
  store.applySession({ sessionEpoch: 2, profile: { uid: 'second' } })
  store.recordFavorite(111, true, 1)
  assert.equal(store.favoriteStatuses.has(111), false)
  store.applySession({ sessionEpoch: 1, profile: { uid: 'first' } })
  assert.equal(store.userProfile.uid, 'second')
})

test('a list request started before a favorite mutation cannot roll its result back', () => {
  const store = useStore(createPinia())
  store.applySession({ sessionEpoch: 1, profile: { uid: 'first' } })
  const requestVersion = store.favoriteRevision
  store.applyFavoriteMutation(111, true, 1)
  store.recordFavorite(111, false, 1, requestVersion)
  assert.equal(store.favoriteStatuses.get(111), true)
  store.recordFavorite(111, false, 1, store.favoriteRevision)
  assert.equal(store.favoriteStatuses.get(111), false)
})
