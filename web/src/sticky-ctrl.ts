/**
 * Converts a typed character into its ASCII control sequence when sticky Ctrl is latched.
 * Supports a-z / A-Z (\x01..\x1a), '[' (\x1b), '\\' (\x1c), ']' (\x1d), and Space (\x00).
 */
export function applyStickyCtrl(inputChar: string): string | null {
  if (!inputChar || inputChar.length === 0) return null;
  const ch = inputChar.charAt(0);
  const code = ch.charCodeAt(0);

  // a-z -> 1..26
  if (code >= 97 && code <= 122) {
    return String.fromCharCode(code - 96);
  }
  // A-Z -> 1..26
  if (code >= 65 && code <= 90) {
    return String.fromCharCode(code - 64);
  }

  switch (ch) {
    case "[":
      return "\x1b"; // ESC
    case "\\":
      return "\x1c"; // FS
    case "]":
      return "\x1d"; // GS
    case " ":
    case "@":
      return "\x00"; // NUL
    default:
      return null;
  }
}
