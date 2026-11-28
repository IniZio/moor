package web

import (
	"slices"
	"sync"

	"github.com/IniZio/moor/internal/domain"
)

// seatNotice is the text message that tells a viewer who has the keyboard.
type seatNotice struct {
	Seat     string   `json:"seat"`
	Holder   string   `json:"holder"`
	Watchers []string `json:"watchers"`
}

const (
	seatSteer = "steer"
	seatWatch = "watch"
)

// viewer is one terminal connection on this replica. notices holds the newest notice only. dropped
// closes when another connection takes the seat from it.
type viewer struct {
	person  string
	notices chan seatNotice
	dropped chan struct{}
}

// seats keeps, per task, the connections on this replica and the one that holds the steering seat.
// Every connection of a task meets here, since each reaches the replica that holds the machine's link.
type seats struct {
	mu    sync.Mutex
	rooms map[domain.TaskID]*room
}

type room struct {
	holder  *viewer
	viewers []*viewer
}

// join adds a connection. One that steers takes the seat, and the one that had it is dropped.
func (s *seats) join(id domain.TaskID, person string, steer bool) *viewer {
	v := &viewer{person: person, notices: make(chan seatNotice, 1), dropped: make(chan struct{})}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.rooms == nil {
		s.rooms = map[domain.TaskID]*room{}
	}
	r := s.rooms[id]
	if r == nil {
		r = &room{}
		s.rooms[id] = r
	}
	r.viewers = append(r.viewers, v)
	if steer {
		if r.holder != nil {
			close(r.holder.dropped)
		}
		r.holder = v
	}
	r.tell()
	return v
}

func (s *seats) leave(id domain.TaskID, v *viewer) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.rooms[id]
	r.viewers = slices.DeleteFunc(r.viewers, func(o *viewer) bool { return o == v })
	if r.holder == v {
		r.holder = nil
	}
	if len(r.viewers) == 0 {
		delete(s.rooms, id)
		return
	}
	r.tell()
}

// steering is whether v holds the seat.
func (s *seats) steering(id domain.TaskID, v *viewer) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.rooms[id]
	return r != nil && r.holder == v
}

// holder names the person who holds the seat, empty when nobody does.
func (s *seats) holder(id domain.TaskID) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	if r := s.rooms[id]; r != nil && r.holder != nil {
		return r.holder.person
	}
	return ""
}

// tell sends every connection the seat as it now stands, replacing a notice it has not read yet.
func (r *room) tell() {
	n := seatNotice{Watchers: []string{}}
	if r.holder != nil {
		n.Holder = r.holder.person
	}
	for _, v := range r.viewers {
		if v != r.holder && !slices.Contains(n.Watchers, v.person) {
			n.Watchers = append(n.Watchers, v.person)
		}
	}
	for _, v := range r.viewers {
		n.Seat = seatWatch
		if v == r.holder {
			n.Seat = seatSteer
		}
		select {
		case <-v.notices:
		default:
		}
		v.notices <- n
	}
}
