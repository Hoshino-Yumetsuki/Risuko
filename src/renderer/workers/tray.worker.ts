/* eslint no-unused-vars: 'off' */
import { TRAY_CANVAS_CONFIG } from '@shared/constants'
import { draw } from '@shared/utils/tray'

let idx = 0
let canvas: OffscreenCanvas | undefined

const initCanvas = () => {
  if (canvas) {
    return canvas
  }

  const { WIDTH, HEIGHT } = TRAY_CANVAS_CONFIG
  return new OffscreenCanvas(WIDTH, HEIGHT)
}

const drawTray = async (payload: any) => {
  self.postMessage({
    type: 'log',
    payload,
  })

  if (!canvas) {
    canvas = initCanvas()
  }

  try {
    await draw({
      canvas,
      ...payload,
    })

    // Read raw RGBA pixels for Tauri `Image::new_owned`.
    const ctx = canvas.getContext('2d')!
    const imageData = ctx.getImageData(0, 0, canvas.width, canvas.height)

    self.postMessage({
      type: 'tray:drawed',
      payload: {
        idx,
        rgba: Array.from(imageData.data),
        width: canvas.width,
        height: canvas.height,
      },
    })

    idx += 1
  } catch (error: any) {
    logger(error.message)
  }
}

const logger = (text: string) => {
  self.postMessage({
    type: 'log',
    payload: text,
  })
}

self.postMessage({
  type: 'initialized',
  payload: Date.now(),
})

self.addEventListener('message', (event) => {
  const { type, payload } = event.data
  switch (type) {
    case 'tray:draw':
      drawTray(payload)
      break
    default:
      logger(JSON.stringify(event.data))
  }
})
