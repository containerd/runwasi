# Documentation website

This project uses the [mdBook]() tool to generate a documentation website from
markdown files. The website is hosted on GitHub Pages and is available at the
following URL: [https://containerd.github.io/runwasi/](https://containerd.github.io/runwasi/).

## Building the documentation

To build the documentation, you need to have the `mdbook` tool installed. You
can install it using the following command:

```bash
cargo install mdbook
```

Once you have `mdbook` installed, you can build the documentation by running the
following command:

```bash
mdbook build
```

This will generate the documentation in the `book` directory. You can verify 
locally by running:

```bash
mdbook serve
```
which will start a local web server at `http://localhost:3000` where you can
view the documentation.

## Contributing

If you would like to contribute to the documentation, you can do so by editing
the markdown files in the `src` directory. Once you have made your changes, you
can build the documentation as described above and verify that your changes are
correct.

If you are happy with your changes, you can submit a pull request to the `main`
branch of the repository. Once your pull request is merged, the changes will be
automatically published to the documentation website.

## Deploying the documentation

The documentation is automatically deployed to GitHub Pages when changes are
merged to the `main` branch.
To deploy the documentation, the following github actions are used:

- [actions-mdbook](https://github.com/peaceiris/actions-mdbook) for building the
  documentation.
- [actions-gh-pages](https://github.com/peaceiris/actions-gh-pages) for
  deploying the documentation to GitHub Pages.
