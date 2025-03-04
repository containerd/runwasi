// Populate the sidebar
//
// This is a script, and not included directly in the page, to control the total size of the book.
// The TOC contains an entry for each page, so if each page includes a copy of the TOC,
// the total size of the page becomes O(n**2).
class MDBookSidebarScrollbox extends HTMLElement {
    constructor() {
        super();
    }
    connectedCallback() {
        this.innerHTML = '<ol class="chapter"><li class="chapter-item expanded affix "><a href="index.html">Introduction</a></li><li class="chapter-item expanded affix "><li class="part-title">Getting Started</li><li class="chapter-item expanded "><a href="getting-started/installation.html"><strong aria-hidden="true">1.</strong> Installation</a></li><li class="chapter-item expanded "><a href="getting-started/demos.html"><strong aria-hidden="true">2.</strong> Demos</a></li><li class="chapter-item expanded "><a href="getting-started/quickstart.html"><strong aria-hidden="true">3.</strong> Quickstart with Kubernetes</a></li><li class="chapter-item expanded "><a href="windows-getting-started.html"><strong aria-hidden="true">4.</strong> Windows Setup</a></li><li class="chapter-item expanded affix "><li class="part-title">Developer Guide</li><li class="chapter-item expanded "><a href="developer/architecture.html"><strong aria-hidden="true">5.</strong> Architecture Overview</a></li><li class="chapter-item expanded "><a href="oci-decision-flow.html"><strong aria-hidden="true">6.</strong> OCI Integration</a></li><li class="chapter-item expanded "><a href="CONTRIBUTING.html"><strong aria-hidden="true">7.</strong> Contributing</a></li><li class="chapter-item expanded "><a href="developer/docs.html"><strong aria-hidden="true">8.</strong> Documentation Guidelines</a></li><li class="chapter-item expanded "><a href="RELEASE.html"><strong aria-hidden="true">9.</strong> Release Process</a></li><li class="chapter-item expanded "><a href="developer/roadmap.html"><strong aria-hidden="true">10.</strong> Project Roadmap</a></li><li class="chapter-item expanded affix "><li class="part-title">Operational</li><li class="chapter-item expanded "><a href="benchmarks.html"><strong aria-hidden="true">11.</strong> Benchmarks</a></li><li class="chapter-item expanded "><a href="opentelemetry.html"><strong aria-hidden="true">12.</strong> OpenTelemetry Integration</a></li><li class="chapter-item expanded "><a href="resources/troubleshooting.html"><strong aria-hidden="true">13.</strong> Troubleshooting</a></li><li class="chapter-item expanded affix "><li class="part-title">Community</li><li class="chapter-item expanded "><a href="resources/faq.html"><strong aria-hidden="true">14.</strong> FAQ</a></li><li class="chapter-item expanded "><a href="resources/community.html"><strong aria-hidden="true">15.</strong> Community</a></li></ol>';
        // Set the current, active page, and reveal it if it's hidden
        let current_page = document.location.href.toString().split("#")[0];
        if (current_page.endsWith("/")) {
            current_page += "index.html";
        }
        var links = Array.prototype.slice.call(this.querySelectorAll("a"));
        var l = links.length;
        for (var i = 0; i < l; ++i) {
            var link = links[i];
            var href = link.getAttribute("href");
            if (href && !href.startsWith("#") && !/^(?:[a-z+]+:)?\/\//.test(href)) {
                link.href = path_to_root + href;
            }
            // The "index" page is supposed to alias the first chapter in the book.
            if (link.href === current_page || (i === 0 && path_to_root === "" && current_page.endsWith("/index.html"))) {
                link.classList.add("active");
                var parent = link.parentElement;
                if (parent && parent.classList.contains("chapter-item")) {
                    parent.classList.add("expanded");
                }
                while (parent) {
                    if (parent.tagName === "LI" && parent.previousElementSibling) {
                        if (parent.previousElementSibling.classList.contains("chapter-item")) {
                            parent.previousElementSibling.classList.add("expanded");
                        }
                    }
                    parent = parent.parentElement;
                }
            }
        }
        // Track and set sidebar scroll position
        this.addEventListener('click', function(e) {
            if (e.target.tagName === 'A') {
                sessionStorage.setItem('sidebar-scroll', this.scrollTop);
            }
        }, { passive: true });
        var sidebarScrollTop = sessionStorage.getItem('sidebar-scroll');
        sessionStorage.removeItem('sidebar-scroll');
        if (sidebarScrollTop) {
            // preserve sidebar scroll position when navigating via links within sidebar
            this.scrollTop = sidebarScrollTop;
        } else {
            // scroll sidebar to current active section when navigating via "next/previous chapter" buttons
            var activeSection = document.querySelector('#sidebar .active');
            if (activeSection) {
                activeSection.scrollIntoView({ block: 'center' });
            }
        }
        // Toggle buttons
        var sidebarAnchorToggles = document.querySelectorAll('#sidebar a.toggle');
        function toggleSection(ev) {
            ev.currentTarget.parentElement.classList.toggle('expanded');
        }
        Array.from(sidebarAnchorToggles).forEach(function (el) {
            el.addEventListener('click', toggleSection);
        });
    }
}
window.customElements.define("mdbook-sidebar-scrollbox", MDBookSidebarScrollbox);
