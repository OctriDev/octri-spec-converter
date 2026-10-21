package main

import (
	"encoding/json"
	"fmt"
	converter "github.com/octridev/octri-spec-converter/bindings/go"
	"io"
	"os"
)

func main() {
	input, err := io.ReadAll(os.Stdin)
	if err != nil {
		panic(err)
	}
	var envelope struct {
		Requests []converter.Request `json:"requests"`
		ABI      int                 `json:"abiVersion"`
		Action   string              `json:"action"`
		Request  json.RawMessage     `json:"request"`
		Before   converter.Report    `json:"before"`
		After    converter.Report    `json:"after"`
		Report   converter.Report    `json:"report"`
		Format   string              `json:"format"`
	}
	if err = json.Unmarshal(input, &envelope); err != nil {
		panic(err)
	}
	a := converter.Converter{}
	var output json.RawMessage
	wrapped := true
	var request converter.Request
	if envelope.ABI == 1 && os.Getenv("OCTRI_BINDING_RAW") != "1" {
		switch envelope.Action {
		case "convert", "bundle":
			if err = json.Unmarshal(envelope.Request, &request); err != nil {
				panic(err)
			}
			if envelope.Action == "convert" {
				output, err = a.Convert(request)
			} else {
				output, err = a.Bundle(request)
			}
		case "detect":
			if err = json.Unmarshal(envelope.Request, &request); err != nil {
				panic(err)
			}
			var format string
			format, err = a.Detect(request)
			if err == nil {
				output, err = json.Marshal(format)
			}
		case "batch":
			var results []converter.BatchResult
			results, err = a.Batch(envelope.Requests)
			if err == nil {
				output, err = json.Marshal(results)
			}
		case "review":
			output, err = a.Review(envelope.Request)
		case "compare":
			output, err = a.Compare(envelope.Before, envelope.After)
		case "export":
			output, err = a.Export(envelope.Report, envelope.Format)
		case "formats":
			output, err = a.Formats()
		default:
			wrapped = false
			output, err = a.Call(input)
		}
	} else {
		wrapped = false
		output, err = a.Call(input)
	}
	if err != nil {
		panic(err)
	}
	if wrapped {
		fmt.Printf("{\"Ok\":%s}\n", output)
	} else {
		fmt.Println(string(output))
	}
}
