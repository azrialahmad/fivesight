import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { htmlToMarkdown } from './htmlmd';

/**
 * Notes Markdown as HTML. Model output is untrusted: it is always sanitized here before it
 * reaches {@html} or an editor. [m:ss] citations become highlighter chips; in an editor they
 * are one piece you can delete but not type into.
 */
export function renderMarkdown(text: string, editable = false): string {
  const html = DOMPurify.sanitize(
    marked.parse(text.replace(/\[(\d{1,2}:\d{2})\]/g, '<mark class="cite">$1</mark>'), { async: false, breaks: true }) as string
  );
  return editable ? html.replaceAll('<mark class="cite">', '<mark class="cite" contenteditable="false">') : html;
}

/**
 * What a paste holds, as Markdown: formatted text (an Ask answer, a web page, a chat message)
 * keeps its headings, lists and bold. `null` when there's only plain text, or nothing.
 */
export function pastedMarkdown(data: DataTransfer | null): string | null {
  const html = data?.getData('text/html');
  if (!html) return null;
  // A template's content is inert: nothing in it loads or runs while it's read.
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  return htmlToMarkdown(tpl.content);
}
