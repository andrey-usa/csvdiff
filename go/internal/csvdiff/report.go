package csvdiff

import (
	"bytes"
	"compress/gzip"
	_ "embed"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"strings"
)

// template is the report shell, embedded so the binary is the only thing you
// need to ship. It is byte-for-byte the same file the Python, TypeScript and
// Java implementations use.
//
//go:embed report.html
var template string

// Render produces the self-contained HTML report: one file, no network, no
// fonts, no frameworks.
//
// Only differing cells are embedded, and the payload is gzip then base64, which
// the browser decodes natively with DecompressionStream. Pass compress=false to
// embed plain JSON for anything older than about 2023.
func Render(result Result, compress bool) (string, error) {
	raw, err := json.Marshal(result)
	if err != nil {
		return "", fmt.Errorf("cannot serialise the result: %w", err)
	}

	var payload, mode string
	if compress {
		var buf bytes.Buffer
		zw, err := gzip.NewWriterLevel(&buf, gzip.BestCompression)
		if err != nil {
			return "", err
		}
		if _, err := zw.Write(raw); err != nil {
			return "", err
		}
		if err := zw.Close(); err != nil {
			return "", err
		}
		payload = base64.StdEncoding.EncodeToString(buf.Bytes())
		mode = "gzip"
	} else {
		// The payload sits inside a <script> element, so a literal "</" would end it early.
		payload = strings.ReplaceAll(string(raw), "</", `<\/`)
		mode = "json"
	}

	title := result.Meta.A.Name + " vs " + result.Meta.B.Name
	out := strings.ReplaceAll(template, "__TITLE__", escapeHTML(title))
	out = strings.ReplaceAll(out, "__MODE__", mode)
	return strings.ReplaceAll(out, "__PAYLOAD__", payload), nil
}

var htmlEscaper = strings.NewReplacer(
	"&", "&amp;", "<", "&lt;", ">", "&gt;", `"`, "&quot;", "'", "&#x27;",
)

func escapeHTML(s string) string { return htmlEscaper.Replace(s) }
