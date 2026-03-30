import { invoke } from '@tauri-apps/api/core'
import logger from '@shared/utils/logger'
import { toast } from 'vue-sonner'
import { getFileNameFromFile, isMagnetTask } from '@shared/utils'
import { APP_THEME, TASK_STATUS, TEMP_DOWNLOAD_SUFFIX } from '@shared/constants'

const GENERATED_TORRENT_HASH_LENGTHS = new Set([40, 64])
const GENERATED_TORRENT_CLEANUP_RETRY_DELAYS = [0, 250, 500] as const

function joinPath(...parts: string[]): string {
  const joined = parts.filter(Boolean).join('/')
  return joined.replace(/[/\\]+/g, '/')
}

export const hasTempDownloadSuffix = (fullPath = ''): boolean => {
  return `${fullPath || ''}`.toLowerCase().endsWith(TEMP_DOWNLOAD_SUFFIX)
}

export const stripTempDownloadSuffix = (fullPath = ''): string => {
  const value = `${fullPath || ''}`
  if (!hasTempDownloadSuffix(value)) {
    return value
  }
  return value.slice(0, value.length - TEMP_DOWNLOAD_SUFFIX.length)
}

export const showItemInFolder = async (
  fullPath: string,
  { errorMsg, fallbackPath }: { errorMsg?: string; fallbackPath?: string } = {},
) => {
  const revealPath = `${fullPath || ''}`.trim()
  const fallback = `${fallbackPath || ''}`.trim()
  if (!revealPath && !fallback) return

  try {
    await invoke('reveal_in_folder', { path: revealPath || fallback })
  } catch (err) {
    logger.warn(`[Motrix] showItemInFolder fail: ${err}`)

    if (fallback && fallback !== revealPath) {
      try {
        await invoke('reveal_in_folder', { path: fallback })
        return
      } catch (fallbackErr) {
        logger.warn(`[Motrix] showItemInFolder fallback fail: ${fallbackErr}`)
      }
    }

    if (errorMsg) {
      toast.error(errorMsg)
    }
  }
}

export const openItem = async (fullPath: string) => {
  if (!fullPath) return
  return invoke('open_path', { path: fullPath })
}

export const getTaskFullPath = (
  task: any,
  options: { normalizeCompletedPath?: boolean } = {},
): string => {
  const { normalizeCompletedPath = true } = options
  const { dir, files, bittorrent } = task
  let result = dir

  if (isMagnetTask(task)) {
    return result
  }

  const isBtMultiFile = !!bittorrent?.info?.name && Array.isArray(files) && files.length > 1
  if (isBtMultiFile) {
    return joinPath(result, bittorrent.info.name)
  }

  const [file] = files
  const path = file.path || ''

  if (path) {
    result = path
  } else if (files?.length === 1) {
    const fileName = getFileNameFromFile(file)
    if (fileName) {
      result = joinPath(result, fileName)
    }
  }

  if (normalizeCompletedPath && task?.status === TASK_STATUS.COMPLETE) {
    return stripTempDownloadSuffix(result)
  }

  return result
}

export const getTaskRevealPath = (task: any): string => {
  if (!task) {
    return ''
  }

  if (isMagnetTask(task)) {
    return `${task?.dir || ''}`.trim()
  }

  const files = Array.isArray(task?.files) ? task.files : []
  const candidate = `${files.find((file: any) => `${file?.path || ''}`.trim())?.path || ''}`.trim()
  if (!candidate) {
    return getTaskFullPath(task)
  }

  if (task?.status === TASK_STATUS.COMPLETE) {
    return stripTempDownloadSuffix(candidate)
  }

  return candidate
}

export const finalizeCompletedDownloadPath = async (task: any): Promise<string> => {
  if (!task) {
    return ''
  }

  if (isMagnetTask(task)) {
    return getTaskFullPath(task)
  }

  const sourcePath = getTaskFullPath(task, {
    normalizeCompletedPath: false,
  })
  if (!hasTempDownloadSuffix(sourcePath)) {
    return sourcePath
  }

  const targetPath = stripTempDownloadSuffix(sourcePath)
  if (!targetPath || targetPath === sourcePath) {
    return sourcePath
  }

  try {
    await invoke('rename_path', {
      fromPath: sourcePath,
      toPath: targetPath,
    })
    return targetPath
  } catch (err) {
    logger.warn(`[Motrix] rename completed temp file failed: ${err}`)
    return sourcePath
  }
}

export const moveTaskFilesToTrash = async (task: any): Promise<boolean> => {
  const { dir, status } = task
  const filesToCleanup = new Set<string>()
  const addCleanupPath = (candidate = '') => {
    const value = `${candidate || ''}`.trim()
    if (!value) return
    filesToCleanup.add(value)
  }

  if (!isMagnetTask(task)) {
    const path = getTaskFullPath(task)
    if (!path || dir === path) {
      throw new Error('task.file-path-error')
    }

    let removedMainFile = true
    try {
      await invoke('trash_item', { path })
    } catch (err) {
      logger.warn(`[Motrix] trash ${path} failed: ${err}`)
      removedMainFile = false
    }

    if (!removedMainFile) {
      return false
    }

    if (status !== TASK_STATUS.COMPLETE) {
      addCleanupPath(`${path}.aria2`)
    }
  }

  for (const cleanupPath of filesToCleanup) {
    try {
      await invoke('trash_item', { path: cleanupPath })
    } catch (err) {
      logger.warn(`[Motrix] trash ${cleanupPath} failed: ${err}`)
    }
  }

  await cleanupGeneratedTorrentSidecars(task)

  return true
}

const resolveGeneratedTorrentSidecarPayload = (task: any) => {
  const normalizeInfoHash = (value = '') => {
    const normalized = `${value || ''}`
      .trim()
      .toLowerCase()
      .replace(/^urn:btih:/, '')
    return normalized.replace(/[^a-f0-9]/g, '')
  }

  const normalizedInfoHash = normalizeInfoHash(
    `${task?.infoHash || task?.bittorrent?.infoHash || ''}`,
  )
  if (!normalizedInfoHash) {
    return null
  }
  if (!GENERATED_TORRENT_HASH_LENGTHS.has(normalizedInfoHash.length)) {
    return null
  }

  const normalizedDir = `${task?.dir || ''}`.trim()
  const sidecarDir = (() => {
    if (normalizedDir) {
      return normalizedDir
    }
    const fullPath = getTaskFullPath(task)
    const normalizedPath = `${fullPath || ''}`.trim()
    if (!normalizedPath) {
      return ''
    }
    const segments = normalizedPath.split(/[/\\]/)
    segments.pop()
    return segments.join('/')
  })()

  if (!sidecarDir) {
    return null
  }

  return {
    dir: sidecarDir,
    infoHash: normalizedInfoHash,
  }
}

const getParentPath = (fullPath = '') => {
  const normalizedPath = `${fullPath || ''}`.trim()
  if (!normalizedPath) {
    return ''
  }
  const segments = normalizedPath.split(/[/\\]/)
  segments.pop()
  return segments.join('/')
}

const buildGeneratedTorrentCandidatePaths = (
  task: any,
  payload: { dir: string; infoHash: string },
) => {
  const candidates = new Set<string>()
  const normalizedHash = payload.infoHash.toLowerCase()
  const fileNames = [`${normalizedHash}.torrent`, `${normalizedHash.toUpperCase()}.torrent`]

  const baseDirs = new Set<string>()
  baseDirs.add(`${payload.dir || ''}`.trim())
  baseDirs.add(`${task?.dir || ''}`.trim())
  baseDirs.add(getParentPath(getTaskFullPath(task, { normalizeCompletedPath: false })))
  baseDirs.add(getParentPath(getTaskFullPath(task)))

  for (const baseDir of baseDirs) {
    if (!baseDir) {
      continue
    }
    for (const fileName of fileNames) {
      candidates.add(joinPath(baseDir, fileName))
    }
  }

  return [...candidates]
}

const fallbackTrashGeneratedTorrentByExactPath = async (
  task: any,
  payload: { dir: string; infoHash: string },
): Promise<number> => {
  const candidatePaths = buildGeneratedTorrentCandidatePaths(task, payload)
  if (candidatePaths.length === 0) {
    return 0
  }

  let deleted = 0
  for (const path of candidatePaths) {
    try {
      await invoke('trash_item', { path })
      deleted += 1
    } catch {
      // Ignore candidate misses; this is best-effort fallback cleanup.
    }
  }
  return deleted
}

export const cleanupGeneratedTorrentSidecars = async (task: any): Promise<number> => {
  const payload = resolveGeneratedTorrentSidecarPayload(task)
  if (!payload) {
    return 0
  }

  let deleted = 0

  for (const delayMs of GENERATED_TORRENT_CLEANUP_RETRY_DELAYS) {
    if (delayMs > 0) {
      await new Promise((resolve) => setTimeout(resolve, delayMs))
    }

    try {
      const result = await invoke<number>('trash_generated_torrent_sidecars', payload)
      const deletedByScan = Number.isFinite(result) ? Number(result) : 0
      deleted += deletedByScan
    } catch (err) {
      logger.warn(`[Motrix] cleanup generated torrent sidecars failed: ${err}`)
    }

    if (deleted > 0) {
      return deleted
    }

    const deletedByFallback = await fallbackTrashGeneratedTorrentByExactPath(task, payload)
    deleted += deletedByFallback
    if (deleted > 0) {
      return deleted
    }
  }
  return deleted
}

export const getSystemTheme = (): string => {
  if (window.matchMedia?.('(prefers-color-scheme: dark)').matches) {
    return APP_THEME.DARK
  }
  return APP_THEME.LIGHT
}

export const delayDeleteTaskFiles = (task: any, delay: number): Promise<boolean> => {
  return new Promise((resolve, reject) => {
    setTimeout(async () => {
      try {
        const result = await moveTaskFilesToTrash(task)
        resolve(result)
      } catch (err: any) {
        reject(err.message)
      }
    }, delay)
  })
}
