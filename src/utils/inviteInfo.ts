export interface ParsedInviteInfo {
  deviceCode: string
  temporaryPassword: string
}

function normalizeCode(value: string): string {
  return value.replace(/[^a-zA-Z0-9]/g, '')
}

function isValidParsedInvite(deviceCode: string, temporaryPassword: string): boolean {
  return deviceCode.length >= 6 && temporaryPassword.length >= 4
}

export function parseInviteInfo(input: string): ParsedInviteInfo | null {
  const text = input.trim()
  if (!text) return null

  const codeMatch = text.match(/(?:设备代码|Device\s*code)\s*[:：]\s*([a-zA-Z0-9 -]+)/i)
  const passwordMatch = text.match(/(?:临时密码|Temporary\s*password)\s*[:：]\s*([a-zA-Z0-9 -]+)/i)
  if (codeMatch?.[1] && passwordMatch?.[1]) {
    const deviceCode = normalizeCode(codeMatch[1])
    const temporaryPassword = normalizeCode(passwordMatch[1])
    return isValidParsedInvite(deviceCode, temporaryPassword)
      ? { deviceCode, temporaryPassword }
      : null
  }

  const tokens = text
    .replace(/设备代码|临时密码|Device\s*code|Temporary\s*password/gi, ' ')
    .match(/[a-zA-Z0-9]+/g) || []
  if (tokens.length < 2) return null

  const [firstToken, secondToken] = tokens
  if (!firstToken || !secondToken) return null

  const deviceCode = normalizeCode(firstToken)
  const temporaryPassword = normalizeCode(secondToken)
  return isValidParsedInvite(deviceCode, temporaryPassword)
    ? { deviceCode, temporaryPassword }
    : null
}
