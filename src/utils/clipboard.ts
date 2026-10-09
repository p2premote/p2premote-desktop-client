/**
 * Copy text in both secure browser contexts and the HTTP Web admin UI.
 *
 * The asynchronous Clipboard API is restricted to secure contexts. Keep the
 * legacy copy command as a synchronous, user-gesture fallback for deployments
 * that intentionally expose the Web admin UI over HTTP.
 */
export async function copyTextToClipboard(text: string): Promise<void> {
  if (!text) throw new Error('Nothing to copy')

  if (window.isSecureContext && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text)
      return
    } catch {
      // A browser policy can still reject the async API. Try the selection
      // based fallback before reporting the copy as failed.
    }
  }

  const textarea = document.createElement('textarea')
  textarea.value = text
  textarea.readOnly = true
  textarea.setAttribute('aria-hidden', 'true')
  textarea.style.position = 'fixed'
  textarea.style.inset = '0 auto auto -9999px'
  textarea.style.opacity = '0'

  const activeElement = document.activeElement instanceof HTMLElement
    ? document.activeElement
    : null
  const selection = window.getSelection()
  const ranges = selection
    ? Array.from({ length: selection.rangeCount }, (_, index) => selection.getRangeAt(index).cloneRange())
    : []

  document.body.appendChild(textarea)
  try {
    textarea.focus()
    textarea.select()
    textarea.setSelectionRange(0, textarea.value.length)
    if (!document.execCommand('copy')) throw new Error('Clipboard copy failed')
  } finally {
    textarea.remove()
    if (selection) {
      selection.removeAllRanges()
      for (const range of ranges) selection.addRange(range)
    }
    activeElement?.focus()
  }
}
