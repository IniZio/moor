package domain

import (
	"fmt"
	"strconv"
	"strings"
)

const cronFields = 5

type cronBound struct {
	name   string
	lo, hi int
	names  []string
	first  int
}

// The five cron fields in order. A name such as mon or jan stands for the number first + its index.
var cronBounds = [cronFields]cronBound{
	{name: "minute", lo: 0, hi: 59},
	{name: "hour", lo: 0, hi: 23},
	{name: "day of month", lo: 1, hi: 31},
	{name: "month", lo: 1, hi: 12, first: 1, names: strings.Fields("jan feb mar apr may jun jul aug sep oct nov dec")},
	{name: "day of week", lo: 0, hi: 7, names: strings.Fields("sun mon tue wed thu fri sat")},
}

// CheckCron accepts five fields of *, numbers, ranges a-b, steps /n, lists and month and weekday names.
func CheckCron(expr string) error {
	fields := strings.Fields(expr)
	if len(fields) != cronFields {
		return fmt.Errorf("cron %q is not %d fields", expr, cronFields)
	}
	for i, f := range fields {
		if err := cronBounds[i].checkField(f); err != nil {
			return fmt.Errorf("cron %q: %w", expr, err)
		}
	}
	return nil
}

func (b cronBound) checkField(field string) error {
	for _, item := range strings.Split(field, ",") {
		if err := b.checkItem(item); err != nil {
			return fmt.Errorf("%s %q: %w", b.name, field, err)
		}
	}
	return nil
}

func (b cronBound) checkItem(item string) error {
	span, step, stepped := strings.Cut(item, "/")
	if stepped {
		if n, ok := digits(step); !ok || n < 1 {
			return fmt.Errorf("step %q is not a positive number", step)
		}
	}
	if span == "*" {
		return nil
	}
	from, to, ranged := strings.Cut(span, "-")
	a, err := b.value(from)
	if err != nil || !ranged {
		return err
	}
	c, err := b.value(to)
	if err == nil && a > c {
		return fmt.Errorf("range %q runs backwards", span)
	}
	return err
}

func (b cronBound) value(s string) (int, error) {
	for i, name := range b.names {
		if strings.EqualFold(s, name) {
			return b.first + i, nil
		}
	}
	n, ok := digits(s)
	if !ok || n < b.lo || n > b.hi {
		return 0, fmt.Errorf("%q is not %d to %d", s, b.lo, b.hi)
	}
	return n, nil
}

func digits(s string) (int, bool) {
	for _, r := range s {
		if r < '0' || r > '9' {
			return 0, false
		}
	}
	n, err := strconv.Atoi(s)
	return n, err == nil
}

func normalizeCron(expr string) string { return strings.Join(strings.Fields(expr), " ") }
