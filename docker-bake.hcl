# special target: https://github.com/docker/metadata-action#bake-definition
target "meta-helper" {}

group "default" {
    targets = ["image"]
}

target "image" {
    inherits = ["meta-helper"]
    output = ["type=image"]
}

target "image-cross" {
    inherits = ["image"]
    platforms = [
        "linux/amd64",
        "linux/arm64"
    ]
}
