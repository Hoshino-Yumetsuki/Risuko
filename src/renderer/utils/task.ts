import { isEmpty } from 'lodash'

import { ADD_TASK_TYPE, NONE_SELECTED_FILES, SELECTED_ALL_FILES } from '@shared/constants'
import { splitTaskLinks } from '@shared/utils'
import { buildOuts } from '@shared/utils/rename'

import {
  buildUrisFromCurl,
  buildHeadersFromCurl,
  buildDefaultOptionsFromCurl,
} from '@shared/utils/curl'

export const initTaskForm = (state: any) => {
  const { addTaskUrl, addTaskOptions } = state.app
  const {
    allProxy,
    cookie,
    dir,
    engineMaxConnectionPerServer,
    followMetalink,
    followTorrent,
    maxConnectionPerServer,
    newTaskShowDownloading,
    referer,
    split,
    userAgent,
  } = state.preference.config
  const splitNumber = Number(split)
  const normalizedSplit =
    Number.isFinite(splitNumber) && splitNumber > 0
      ? Math.max(1, Math.min(Math.trunc(splitNumber), 16))
      : 16

  const result = {
    allProxy,
    cookie: cookie || '',
    dir,
    engineMaxConnectionPerServer,
    followMetalink,
    followTorrent,
    maxConnectionPerServer,
    newTaskShowDownloading,
    out: '',
    referer: referer || '',
    selectFile: NONE_SELECTED_FILES,
    split: normalizedSplit,
    torrentPath: '',
    uris: addTaskUrl,
    userAgent: userAgent || '',
    authorization: '',
    ...addTaskOptions,
  }
  return result
}

const buildHeader = (form: any) => {
  const { cookie, authorization } = form
  const result = []
  if (!isEmpty(cookie)) {
    result.push(`Cookie: ${cookie}`)
  }
  if (!isEmpty(authorization)) {
    result.push(`Authorization: ${authorization}`)
  }

  return result
}

const buildOption = (type: string, form: any) => {
  const { allProxy, dir, out, referer, selectFile, split, userAgent } = form
  const result: any = {}

  if (!isEmpty(allProxy)) {
    result.allProxy = allProxy
  }

  if (!isEmpty(dir)) {
    result.dir = dir
  }

  if (!isEmpty(out)) {
    result.out = out
  }

  if (split > 0) {
    result.split = split
  }

  if (!isEmpty(userAgent)) {
    result.userAgent = userAgent
  }

  if (!isEmpty(referer)) {
    result.referer = referer
  }

  if (type === ADD_TASK_TYPE.TORRENT) {
    const normalizedSelectFile = `${selectFile || ''}`.trim()
    const hasExplicitSelection =
      normalizedSelectFile &&
      normalizedSelectFile !== SELECTED_ALL_FILES &&
      normalizedSelectFile !== NONE_SELECTED_FILES
    if (hasExplicitSelection) {
      result.selectFile = normalizedSelectFile
    }
  }

  const header = buildHeader(form)
  if (!isEmpty(header)) {
    result.header = header
  }

  return result
}

export const buildUriPayload = (form: any) => {
  let { uris, out } = form
  if (isEmpty(uris)) {
    throw new Error('task.new-task-uris-required')
  }

  uris = splitTaskLinks(uris)
  const curlHeaders = buildHeadersFromCurl(uris)
  uris = buildUrisFromCurl(uris)
  const outs = buildOuts(uris, out)

  form = buildDefaultOptionsFromCurl(form, curlHeaders)

  const options = buildOption(ADD_TASK_TYPE.URI, form)
  const result = {
    uris,
    outs,
    options,
  }
  return result
}

export const buildTorrentPayload = (form: any) => {
  const { torrentPath } = form
  if (isEmpty(torrentPath)) {
    throw new Error('task.new-task-torrent-required')
  }

  const options = buildOption(ADD_TASK_TYPE.TORRENT, form)
  const result = {
    torrentPath,
    options,
  }
  return result
}
