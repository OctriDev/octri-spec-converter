// Package converter calls the Octri Rust engine through its versioned C ABI.
package converter

/*
#cgo LDFLAGS: -loctri_converter_ffi
#include "octri_converter.h"
*/
import "C"

import (
	"encoding/json"
	"errors"
	"runtime"
	"unsafe"
)

type Source struct {
	URI       string `json:"uri"`
	Content   string `json:"content"`
	Generated bool   `json:"generated"`
}

func Text(content, uri string) Source { return Source{URI: uri, Content: content} }

type Request struct {
	Input  Source            `json:"input"`
	Files  map[string]string `json:"files,omitempty"`
	Config json.RawMessage   `json:"config,omitempty"`
}

// Reports retain exact JSON numbers and every evolving versioned field.
// Decode into application types without rounding uint64 into float64.
type Report = json.RawMessage

type BatchResult struct {
	Ok  Report           `json:"Ok,omitempty"`
	Err *ConversionError `json:"Err,omitempty"`
}

type ConversionError struct {
	Code        string          `json:"code"`
	Message     string          `json:"message"`
	Diagnostics json.RawMessage `json:"diagnostics"`
}

func (e *ConversionError) Error() string { return e.Message }

type Converter struct{}

func (Converter) Call(envelope json.RawMessage) (json.RawMessage, error) {
	if C.octri_converter_abi_version() != 1 {
		return nil, errors.New("expected converter ABI version 1")
	}
	if len(envelope) > 128*1024*1024 {
		return nil, errors.New("converter envelope exceeds 128 MiB")
	}
	var input *C.uint8_t
	if len(envelope) > 0 {
		input = (*C.uint8_t)(unsafe.Pointer(&envelope[0]))
	}
	output := C.octri_converter_call(input, C.size_t(len(envelope)))
	runtime.KeepAlive(envelope)
	if output == nil {
		return nil, errors.New("native converter returned a null buffer")
	}
	defer C.octri_converter_buffer_free(output)
	length := C.octri_converter_buffer_len(output)
	if uint64(length) > uint64(^uint(0)>>1) {
		return nil, errors.New("native output exceeds Go slice limit")
	}
	bytes := unsafe.Slice((*byte)(unsafe.Pointer(C.octri_converter_buffer_data(output))), int(length))
	return append(json.RawMessage(nil), bytes...), nil
}

func (a Converter) result(action string, fields map[string]interface{}) (json.RawMessage, error) {
	fields["abiVersion"] = 1
	fields["action"] = action
	encoded, err := json.Marshal(fields)
	if err != nil {
		return nil, err
	}
	output, err := a.Call(encoded)
	if err != nil {
		return nil, err
	}
	var result struct {
		Ok  json.RawMessage
		Err *ConversionError
	}
	if err = json.Unmarshal(output, &result); err != nil {
		return nil, err
	}
	if result.Err != nil {
		return nil, result.Err
	}
	if result.Ok == nil {
		return nil, errors.New("invalid converter response envelope")
	}
	return result.Ok, nil
}
func (a Converter) Convert(request Request) (Report, error) {
	return a.result("convert", map[string]interface{}{"request": request})
}
func (a Converter) Review(request json.RawMessage) (json.RawMessage, error) {
	return a.result("review", map[string]interface{}{"request": request})
}
func (a Converter) Compare(before, after Report) (json.RawMessage, error) {
	return a.result("compare", map[string]interface{}{"before": before, "after": after})
}
func (a Converter) Export(report Report, format string) (json.RawMessage, error) {
	return a.result("export", map[string]interface{}{"report": report, "format": format})
}
func (a Converter) Formats() (json.RawMessage, error) {
	return a.result("formats", map[string]interface{}{})
}
func (a Converter) Detect(request Request) (string, error) {
	value, err := a.result("detect", map[string]interface{}{"request": request})
	if err != nil {
		return "", err
	}
	var format string
	err = json.Unmarshal(value, &format)
	return format, err
}
func (a Converter) Batch(requests []Request) ([]BatchResult, error) {
	value, err := a.result("batch", map[string]interface{}{"requests": requests})
	if err != nil {
		return nil, err
	}
	var results []BatchResult
	err = json.Unmarshal(value, &results)
	return results, err
}
func (a Converter) Bundle(request Request) (json.RawMessage, error) {
	return a.result("bundle", map[string]interface{}{"request": request})
}
