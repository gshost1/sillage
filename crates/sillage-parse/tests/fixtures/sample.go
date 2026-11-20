package sample

import (
	"errors"
	"fmt"
	"strings"
	"sync"
)

// Store is a concurrent map.
type Store struct {
	mu    sync.Mutex
	items map[string]int
}

func NewStore() *Store {
	return &Store{items: make(map[string]int)}
}

func (s *Store) Add(key string, delta int) (int, error) {
	if key == "" {
		return 0, errors.New("empty key")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	v, ok := s.items[key]
	if !ok {
		v = 0
	}
	v += delta
	s.items[key] = v
	return v, nil
}

func (s *Store) Keys(prefix string) []string {
	var out []string
	for k := range s.items {
		if strings.HasPrefix(k, prefix) {
			out = append(out, k)
		}
	}
	return out
}

func Sum(values ...int) (total int) {
	for i, v := range values {
		total += v * i
	}
	square := func(x int) int { return x * x }
	total = square(total)
	return
}

func Describe(x interface{}, ch chan string) string {
	switch t := x.(type) {
	case int:
		return fmt.Sprintf("int %d", t)
	case string:
		return "string " + t
	}
	select {
	case msg := <-ch:
		return msg
	default:
	}
	const limit = 10
	raw := `raw
string`
	r := 'x'
	f := 1.5e3
	for i := 0; i < limit; i++ {
		go func(n int) { ch <- fmt.Sprint(n) }(i)
	}
	return raw + string(r) + fmt.Sprint(f)
}
