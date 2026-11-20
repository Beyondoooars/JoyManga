import test from 'node:test'
import assert from 'node:assert/strict'
import { compareVersions } from '../src/utils/semver.ts'

test('compareVersions compares core versions and v prefix', () => {
  assert.equal(compareVersions('v0.3.18', '0.3.18'), 0)
  assert.equal(compareVersions('0.4.0', '0.3.19'), 1)
  assert.equal(compareVersions('0.3.18', '0.3.19'), -1)
  assert.equal(compareVersions('1.2', '1.2.0'), 0)
})

test('compareVersions handles prerelease precedence per SemVer 2.0.0', () => {
  assert.equal(compareVersions('1.0.0', '1.0.0-beta.1'), 1)
  assert.equal(compareVersions('1.0.0-beta.1', '1.0.0'), -1)
  assert.equal(compareVersions('1.0.0-alpha.2', '1.0.0-alpha.10'), -1)
  assert.equal(compareVersions('1.0.0-1', '1.0.0-alpha'), -1)
  assert.equal(compareVersions('1.0.0-beta', '1.0.0-beta.1'), -1)
  assert.equal(compareVersions('1.0.0-rc.1', '1.0.0-beta.9'), 1)
})

test('compareVersions ignores build metadata', () => {
  assert.equal(compareVersions('1.2.3+build.1', '1.2.3+build.99'), 0)
  assert.equal(compareVersions('1.2.3-rc.1+20260101', '1.2.3-rc.1+20260201'), 0)
})
