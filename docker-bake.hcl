variable "CRATE" {
    default = ""
}

# special target: https://github.com/docker/metadata-action#bake-definition
target "meta-helper" {}

group "default" {
    targets = ["image"]
}

target "image" {
    inherits = ["meta-helper"]
    output = ["type=image"]
    args = {
        "CRATE" = CRATE
    }
}

target "image-cross" {
    inherits = ["image"]
    platforms = [
        "linux/amd64",
        "linux/arm64"
    ]
}

target "bins" {
    inherits = ["image"]
    output= ["type=local,dest=bin/"]
}

target "bins-cross" {
    inherits = ["bins"]
    platforms = [
        "linux/amd64",
        "linux/arm64"
    ]
}

target "release-tars" {
    inherits = ["bins-cross"]
    output = ["type=local,dest=release/"]
    target = "release-tar"
}