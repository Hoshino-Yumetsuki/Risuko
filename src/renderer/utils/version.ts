import { getVersion } from '@tauri-apps/api/app'

export const getMotrixVersion = async (): Promise<string> => {
  try {
    const version = await getVersion()
    return version ? `v${version}` : ''
  } catch {
    return ''
  }
}
