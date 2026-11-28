// Command portal serves the product: pages, webhooks, the product MCP and runner links.
package main

import "fmt"

var version = "dev"

func main() {
	fmt.Println("portal", version)
}
