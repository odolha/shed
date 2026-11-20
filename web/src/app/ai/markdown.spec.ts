import { renderMarkdown, sanitize } from './markdown';

describe('renderMarkdown', () => {
  it('renders the usual Markdown', () => {
    const html = renderMarkdown('# Title\n\n- **one**\n- `two`\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```');
    expect(html).toContain('<h1>Title</h1>');
    expect(html).toContain('<strong>one</strong>');
    expect(html).toContain('<code>two</code>');
    expect(html).toContain('<table>');
    expect(html).toContain('<pre><code class="language-rust">fn main() {}');
  });

  it('removes scripts and event handlers', () => {
    const html = renderMarkdown('hi <script>alert(1)</script><img src="x" onerror="alert(2)"><b onclick="x()">b</b>');
    expect(html).not.toContain('<script');
    expect(html).not.toContain('onerror');
    expect(html).not.toContain('onclick');
    expect(html).toContain('<b>b</b>');
  });

  it('keeps only safe links, opened apart', () => {
    const html = renderMarkdown('[ok](https://example.com) [bad](javascript:alert(1)) [file](src/main.rs)');
    expect(html).toContain('href="https://example.com"');
    expect(html).toContain('target="_blank"');
    expect(html).toContain('rel="noopener noreferrer"');
    expect(html).not.toContain('javascript:');
    expect(html).not.toContain('href="src/main.rs"');
  });

  it('drops styles, forms and frames', () => {
    const html = sanitize('<p style="color:red">x</p><form><input name="a"></form><iframe src="https://e.com"></iframe>');
    expect(html).not.toContain('style=');
    expect(html).not.toContain('<form');
    expect(html).not.toContain('<iframe');
  });

  it('allows inline picture data but not other data URLs', () => {
    const ok = sanitize('<img src="data:image/png;base64,AAAA">');
    const bad = sanitize('<img src="data:text/html;base64,AAAA">');
    expect(ok).toContain('data:image/png');
    expect(bad).not.toContain('data:text/html');
  });
});
