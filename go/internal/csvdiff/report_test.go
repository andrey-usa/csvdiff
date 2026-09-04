package csvdiff

import (
	"encoding/base64"
	"encoding/json"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

var payloadRe = regexp.MustCompile(`(?s)<script id="payload" type="application/(\w+)">(.*?)</script>`)

func sampleResult(t *testing.T) Result {
	t.Helper()
	dir := t.TempDir()
	a := filepath.Join(dir, "a.csv")
	b := filepath.Join(dir, "b.csv")
	os.WriteFile(a, []byte(simpleA), 0o644)
	os.WriteFile(b, []byte(simpleB), 0o644)
	o := opts("id")
	o.EngineName = string(EngineNative)
	res, err := Compare(a, b, o)
	if err != nil {
		t.Fatal(err)
	}
	return res
}

// The report has to work from a file:// URL with no network at all, so nothing
// in it may point outwards.
func TestReportIsSelfContained(t *testing.T) {
	html, err := Render(sampleResult(t), true)
	if err != nil {
		t.Fatal(err)
	}
	for _, forbidden := range []string{"http://", "https://", "<link", "src=\"//"} {
		if strings.Contains(html, forbidden) {
			t.Errorf("the report reaches outside itself: found %q", forbidden)
		}
	}
	for _, placeholder := range []string{"__TITLE__", "__MODE__", "__PAYLOAD__"} {
		if strings.Contains(html, placeholder) {
			t.Errorf("placeholder %s was left unsubstituted", placeholder)
		}
	}
}

func TestGzipPayloadRoundTrips(t *testing.T) {
	want := sampleResult(t)
	html, err := Render(want, true)
	if err != nil {
		t.Fatal(err)
	}
	m := payloadRe.FindStringSubmatch(html)
	if m == nil {
		t.Fatal("no payload element in the report")
	}
	if m[1] != "gzip" {
		t.Fatalf("payload mode = %s, want gzip", m[1])
	}
	if _, err := base64.StdEncoding.DecodeString(m[2]); err != nil {
		t.Fatalf("payload is not base64: %v", err)
	}
	// The compressed form must be worth the trouble.
	plain, err := Render(want, false)
	if err != nil {
		t.Fatal(err)
	}
	if len(html) >= len(plain) {
		t.Errorf("gzip payload (%d bytes) is no smaller than plain JSON (%d bytes)", len(html), len(plain))
	}
}

// An uncompressed payload sits inside a <script>, where a literal "</" would end
// the element early and break the page.
func TestPlainPayloadEscapesScriptEnd(t *testing.T) {
	res := sampleResult(t)
	res.Meta.A.Name = "a</script>x.csv"
	html, err := Render(res, false)
	if err != nil {
		t.Fatal(err)
	}
	m := payloadRe.FindStringSubmatch(html)
	if m == nil {
		t.Fatal("no payload element in the report")
	}
	var decoded Result
	if err := json.Unmarshal([]byte(strings.ReplaceAll(m[2], `<\/`, "</")), &decoded); err != nil {
		t.Fatalf("plain payload is not valid JSON: %v", err)
	}
	if decoded.Meta.A.Name != res.Meta.A.Name {
		t.Errorf("name round-tripped as %q", decoded.Meta.A.Name)
	}
}

func TestTitleIsHtmlEscaped(t *testing.T) {
	res := sampleResult(t)
	res.Meta.A.Name = `<img onerror="x">`
	html, err := Render(res, true)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(html, `<img onerror=`) {
		t.Error("the title was not escaped before being put in the document")
	}
}

// CellDiff is the compact [column, a, b] triple every implementation reads.
func TestCellDiffSerialisesAsATriple(t *testing.T) {
	v := "x"
	got, err := json.Marshal(CellDiff{Column: 2, A: &v, B: nil})
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != `[2,"x",null]` {
		t.Errorf("CellDiff JSON = %s, want [2,\"x\",null]", got)
	}
}
